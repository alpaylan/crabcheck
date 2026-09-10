pub mod analyze;

use std::{
    fmt::Debug,
    fs,
    path::{
        Path,
        PathBuf,
    },
    time::SystemTime,
};

use {
    rand::rngs::ThreadRng,
    serde::Serialize,
};

use crate::quickcheck::{
    Arbitrary,
    Mutate,
    ResultStatus,
    RunResult,
};


extern "C" {
    fn __llvm_profile_write_file() -> i32;
    fn __llvm_profile_reset_counters();
    /// Override the path the runtime writes profraws to. Accepts the same
    /// `%p` / `%m` substitutions as `LLVM_PROFILE_FILE`. We use this from
    /// `quickcheck_with_locate` to route per-call workdirs without forcing
    /// the user to set the env var before starting the binary (which the
    /// runtime would otherwise cache at init).
    fn __llvm_profile_set_filename(name: *const std::os::raw::c_char);
}

/// Wrapper around `__llvm_profile_set_filename`. The string must be a valid
/// `LLVM_PROFILE_FILE`-style pattern; `%p` expands to PID, `%m` to a
/// counter-binary signature.
fn set_profile_filename(pattern: &str) {
    let cstr = match std::ffi::CString::new(pattern) {
        Ok(s) => s,
        Err(_) => return,
    };
    unsafe { __llvm_profile_set_filename(cstr.as_ptr()) };
}


/// Dump the runtime LLVM coverage counters into the most recent `.profraw`
/// file inside `workdir`, then rename it to `<workdir>/snapshot_<label>.profraw`.
pub(crate) fn snapshot(workdir: &Path, label: &str) {
    tracing::debug!("Taking snapshot: {} (workdir={})", label, workdir.display());
    unsafe {
        __llvm_profile_write_file();
    }

    // Glob for the current .profraw inside workdir.
    let pattern = format!("{}/*.profraw", workdir.display());
    let files = match glob::glob(&pattern) {
        Ok(g) => g,
        Err(_) => return,
    };
    let newest: Option<PathBuf> = files.filter_map(Result::ok).max_by_key(|path| {
        path.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH)
    });

    if let Some(path) = newest {
        let new_name = workdir.join(format!("snapshot_{}.profraw", label));
        fs::rename(&path, &new_name).expect("rename failed");
    }
}

pub(crate) fn reset() {
    unsafe { __llvm_profile_reset_counters() };
}

/// Slot shared between the panic hook (writer) and `safe_call` (reader).
/// Holds the most recent panic's (file, line, backtrace) until the wrapped
/// `catch_unwind` returns control to `safe_call`, which drains the slot to
/// `<workdir>/panic_locations.jsonl`. Single-threaded today, so the Mutex is
/// just there to satisfy the `Send + Sync` bound the panic hook closure needs.
type PanicSlot = std::sync::Arc<std::sync::Mutex<Option<(String, u32, String)>>>;

/// Install a process-global panic hook that writes (file, line, backtrace)
/// into `slot`. Returns an RAII guard that restores the previous hook on
/// drop. NOT recursion-safe: do not nest two installations on the same
/// thread; the inner one would steal the slot.
fn install_panic_capture(slot: PanicSlot) -> impl Drop {
    let cap = std::sync::Arc::clone(&slot);
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let bt = std::backtrace::Backtrace::force_capture().to_string();
        let (file, line) =
            info.location().map(|l| (l.file().to_string(), l.line())).unwrap_or_default();
        if let Ok(mut g) = cap.lock() {
            *g = Some((file, line, bt));
        }
    }));
    struct Guard(Option<Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + 'static + Sync + Send>>);
    impl Drop for Guard {
        fn drop(&mut self) {
            if let Some(p) = self.0.take() {
                std::panic::set_hook(p);
            }
        }
    }
    Guard(Some(prev))
}

/// Wrap one property invocation in `catch_unwind`. On panic, drain the slot
/// (populated by the hook installed via `install_panic_capture`) and append
/// a JSONL entry to `<workdir>/panic_locations.jsonl`. Returns `Some(false)`
/// to the caller so the rest of the loop classifies the iteration as a
/// failure without itself panicking.
fn safe_call<T>(
    workdir: &Path,
    f: fn(T) -> Option<bool>,
    input: T,
    slot: &PanicSlot,
) -> Option<bool> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || f(input)))
        .unwrap_or(Some(false));
    let drained = slot.lock().ok().and_then(|mut g| g.take());
    if let Some((file, line, bt)) = drained {
        if !file.is_empty() {
            let entry = serde_json::json!({
                "file": file,
                "line": line,
                "bt": bt,
            })
            .to_string();
            let path = workdir.join("panic_locations.jsonl");
            if let Ok(mut h) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                use std::io::Write;
                let _ = writeln!(h, "{entry}");
            }
        }
    }
    result
}

/// Read an env-var override or fall back to `default`.
fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key).ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

/// Backward-compatible entry point. Behaves identically to
/// `quickcheck_with_shrink` with a no-op shrinker, writing into `coverage/`.
pub fn quickcheck<T: Arbitrary<ThreadRng> + Mutate<ThreadRng> + Clone + Debug>(
    f: fn(T) -> Option<bool>,
) -> RunResult {
    quickcheck_impl(Path::new("coverage"), f, |_| Vec::new())
}

/// Workdir-parameterized variant of [`quickcheck`]. All snapshot files,
/// `indices.json`, and `panic_locations.jsonl` land under `workdir`.
pub fn quickcheck_in<T: Arbitrary<ThreadRng> + Mutate<ThreadRng> + Clone + Debug>(
    workdir: &Path,
    f: fn(T) -> Option<bool>,
) -> RunResult {
    quickcheck_impl(workdir, f, |_| Vec::new())
}

/// Same as `quickcheck`, but after the first failing seed is found, iterate
/// through `shrink(&seed)` candidates looking for smaller still-failing inputs.
/// The minimal failing input is used as the starting point for the
/// snapshot-and-mutate loop, so the coverage signal reflects the bug in its
/// simplest form.
///
/// `shrink(&t)` must return only candidates structurally smaller than `t`
/// (otherwise the loop may not terminate within the safety cap).
pub fn quickcheck_with_shrink<T: Arbitrary<ThreadRng> + Mutate<ThreadRng> + Clone + Debug>(
    f: fn(T) -> Option<bool>,
    shrink: fn(&T) -> Vec<T>,
) -> RunResult {
    quickcheck_impl(Path::new("coverage"), f, shrink)
}

/// Workdir-parameterized variant of [`quickcheck_with_shrink`].
pub fn quickcheck_with_shrink_in<T: Arbitrary<ThreadRng> + Mutate<ThreadRng> + Clone + Debug>(
    workdir: &Path,
    f: fn(T) -> Option<bool>,
    shrink: fn(&T) -> Vec<T>,
) -> RunResult {
    quickcheck_impl(workdir, f, shrink)
}

fn quickcheck_impl<T: Arbitrary<ThreadRng> + Mutate<ThreadRng> + Clone + Debug>(
    workdir: &Path,
    f: fn(T) -> Option<bool>,
    shrink: fn(&T) -> Vec<T>,
) -> RunResult {
    let _ = fs::create_dir_all(workdir); // best-effort; if it fails we'll error later

    let mut rng = rand::rng();
    tracing::debug!("Starting profiling quickcheck in {}...", workdir.display());

    // Knobs, overridable at runtime so we can tune without rebuilding:
    //   CRABCHECK_PROFILING_MUTATIONS       — mutation-loop bound (default 1000)
    //   CRABCHECK_PROFILING_INITIAL_PASSES  — cap on initial-sweep positive
    //                                          snapshots (default 100)
    //   CRABCHECK_PROFILING_RANDOM_ITERS    — outer random-sweep bound (default 20000)
    //   CRABCHECK_PROFILING_MAX_SHRINK_STEPS — shrink-loop safety cap (default 1000)
    //
    // Labels: mutation iterations use indices 1..=MAX_MUTATIONS. Initial-sweep
    // positives use `INITIAL_PASS_BASE + idx` where `INITIAL_PASS_BASE =
    // MAX_MUTATIONS + 1`, so the two ranges never collide regardless of the
    // chosen MAX_MUTATIONS.
    let max_mutations = env_usize("CRABCHECK_PROFILING_MUTATIONS", 1000);
    let max_initial_pass_snapshots = env_usize("CRABCHECK_PROFILING_INITIAL_PASSES", 100);
    let n = env_usize("CRABCHECK_PROFILING_RANDOM_ITERS", 20_000);
    let max_shrink_steps = env_usize("CRABCHECK_PROFILING_MAX_SHRINK_STEPS", 1000);
    let initial_pass_base = max_mutations + 1;

    // Install the panic-capture hook for the duration of this loop. Each
    // wrapped `safe_call` invocation drains the slot to
    // `<workdir>/panic_locations.jsonl` after a panic; the guard restores the
    // previous hook on drop (i.e. when this function returns).
    let slot: PanicSlot = std::sync::Arc::new(std::sync::Mutex::new(None));
    let _hook_guard = install_panic_capture(slot.clone());

    let mut passed = 0;
    let mut discarded = 0;
    let mut initial_passes: Vec<(usize, String)> = Vec::with_capacity(max_initial_pass_snapshots);
    for i in 0..n {
        let input = T::generate(&mut rng, ((i + 1) as f32).log2() as usize);
        tracing::trace!("Test #{}: {:?}", i, input);
        match safe_call(workdir, f, input.clone(), &slot) {
            None => discarded += 1,
            Some(true) => {
                passed += 1;
                if initial_passes.len() < max_initial_pass_snapshots {
                    let label_idx = initial_pass_base + initial_passes.len();
                    crate::profiling::reset();
                    let _ = safe_call(workdir, f, input.clone(), &slot);
                    crate::profiling::snapshot(workdir, format!("iteration_{label_idx}").as_str());
                    initial_passes.push((label_idx, format!("{:?}", input)));
                }
            },
            Some(false) => {
                // Shrink the failing seed to its minimal still-failing form.
                let mut seed = input.clone();
                let mut shrink_steps = 0usize;
                'shrink: loop {
                    let candidates = shrink(&seed);
                    if candidates.is_empty() {
                        break 'shrink;
                    }
                    let mut found_smaller = false;
                    for candidate in candidates {
                        if let Some(false) = safe_call(workdir, f, candidate.clone(), &slot) {
                            seed = candidate;
                            found_smaller = true;
                            break;
                        }
                    }
                    if !found_smaller {
                        break 'shrink;
                    }
                    shrink_steps += 1;
                    if shrink_steps >= max_shrink_steps {
                        tracing::info!("shrink step cap ({max_shrink_steps}) reached");
                        break 'shrink;
                    }
                }
                if shrink_steps > 0 {
                    tracing::info!("shrunk failing input in {shrink_steps} step(s): {:?}", seed);
                }

                let (mut positives, mut negatives) =
                    (initial_passes.clone(), vec![(0, format!("{:?}", seed))]);
                crate::profiling::reset();
                let _ = safe_call(workdir, f, seed.clone(), &slot);
                crate::profiling::snapshot(workdir, "iteration_0");

                for i in 1..=max_mutations {
                    let input = T::mutate(&seed, &mut rng, ((i + 1) as f32).log2() as usize);
                    crate::profiling::reset();
                    let result = safe_call(workdir, f, input.clone(), &slot);
                    crate::profiling::snapshot(workdir, format!("iteration_{i}").as_str());
                    match result {
                        None => discarded += 1,
                        Some(true) => {
                            positives.push((i, format!("{:?}", input)));
                        },
                        Some(false) => {
                            negatives.push((i, format!("{:?}", input)));
                        },
                    }
                }
                tracing::debug!("positives: {}", positives.len());
                tracing::debug!("negatives: {}", negatives.len());

                #[derive(Serialize)]
                struct IndicesConfig {
                    max_mutations: usize,
                    max_initial_passes: usize,
                    initial_pass_base: usize,
                    random_iters: usize,
                    shrink_steps: usize,
                }

                #[derive(Serialize)]
                struct Indices {
                    positives: Vec<usize>,
                    negatives: Vec<usize>,
                    positive_examples: Vec<String>,
                    negative_examples: Vec<String>,
                    config: IndicesConfig,
                }

                let indices = Indices {
                    positives: positives.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
                    negatives: negatives.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
                    positive_examples: positives.iter().map(|(_, s)| s.clone()).collect::<Vec<_>>(),
                    negative_examples: negatives.iter().map(|(_, s)| s.clone()).collect::<Vec<_>>(),
                    config: IndicesConfig {
                        max_mutations,
                        max_initial_passes: max_initial_pass_snapshots,
                        initial_pass_base,
                        random_iters: n,
                        shrink_steps,
                    },
                };

                let json = serde_json::to_string(&indices).unwrap();
                let file_path = workdir.join("indices.json");
                tracing::debug!("JSON:\n{}\n", json);
                fs::write(&file_path, json).expect("Unable to write indices.json");
                tracing::debug!("indices.json written to {}", file_path.display());


                return RunResult {
                    status: ResultStatus::Failed { arguments: vec![format!("{:?}", seed)] },
                    passed,
                    discarded,
                };
            },
        }
    }

    RunResult { passed: n as u64, discarded: 0, status: ResultStatus::Finished }
}

// ===========================================================================
// `quickcheck_with_locate` — runs the property AND locates the bug.
// ===========================================================================

/// Allocate a per-call workdir under `coverage/` derived from `(pid, seq)`.
/// Two concurrent `cargo test` threads calling [`quickcheck_with_locate`]
/// will get distinct workdirs and not race on `panic_locations.jsonl` or the
/// snapshot rename glob.
fn allocate_workdir() -> PathBuf {
    use std::sync::atomic::{
        AtomicUsize,
        Ordering,
    };
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let pid = std::process::id();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    PathBuf::from(format!("coverage/{pid}-{seq}"))
}

/// Bundled output of [`quickcheck_with_locate`]. Carries the original
/// [`RunResult`] alongside the ranked suspects, any setup diagnostics
/// detected during the run, and a low-level error string if the analyzer
/// couldn't run at all.
pub struct LocateResult {
    pub run: RunResult,
    pub suspects: Vec<analyze::RankedRegion>,
    pub n_panics: usize,
    /// User-facing setup / configuration diagnostics. Surfaces specific
    /// failure modes (missing instrumentation, wrong module name, etc.)
    /// with one-line fix hints. The orchestrator populates this from
    /// `analyze::AnalyzeDiagnostics` plus the run's own state.
    pub diagnostics: Vec<analyze::SetupDiagnostic>,
    pub error: Option<String>,
    /// The per-call workdir under `coverage/<pid>-<seq>/`. Useful for
    /// debugging — the user can inspect `coverage/<pid>-<seq>/snapshot_*.profraw`
    /// or feed it to `crabcheck-profiling-fast-analyze` manually.
    pub workdir: PathBuf,
}

impl LocateResult {
    /// The top-ranked suspect, or `None` when the run found no counterexample
    /// or analysis couldn't run.
    pub fn top(&self) -> Option<&analyze::RankedRegion> {
        self.suspects.first()
    }

    /// Convenience for `#[test]` bodies: panic if the run didn't find a bug.
    pub fn assert_failed(&self) {
        assert!(
            matches!(self.run.status, ResultStatus::Failed { .. }),
            "expected a counterexample, got status: {:?}",
            self.run.status
        );
    }
}

impl std::fmt::Display for LocateResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.run.status {
            ResultStatus::Failed { arguments } => {
                writeln!(
                    f,
                    "Failed (passed={}, discarded={})",
                    self.run.passed, self.run.discarded
                )?;
                writeln!(f, "Counterexample: {}", arguments.join(", "))?;
                if self.n_panics > 0 {
                    writeln!(f, "Captured {} panic events.", self.n_panics)?;
                }
                if let Some(err) = &self.error {
                    writeln!(f, "Analyzer error: {err}")?;
                }
                // Surface configuration hints prominently — before the
                // suspect table — so the user doesn't have to puzzle out
                // why the table is empty or wrong.
                for d in &self.diagnostics {
                    writeln!(f, "\nConfiguration issue: {d}")?;
                }
                if self.suspects.is_empty() {
                    if self.diagnostics.is_empty() {
                        writeln!(f, "(no suspects: empty coverage data)")?;
                    }
                    return Ok(());
                }
                writeln!(f, "\nTop suspects:")?;
                for s in self.suspects.iter().take(5) {
                    let prior_tag = if s.panic_overlap { "panic-overlap " } else { "" };
                    let fname = std::path::Path::new(&s.region.file)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(s.region.file.as_str());
                    writeln!(
                        f,
                        "  {}. {}:{}-{} in `{}`  ochiai={:.2} Δ={:.2}  {} {}{}",
                        s.rank,
                        fname,
                        s.region.start_line,
                        s.region.end_line,
                        s.region.function,
                        s.region.suspiciousness.ochiai,
                        s.region.delta,
                        s.confidence,
                        prior_tag,
                        format_args!("({})", s.confidence_rule),
                    )?;
                }
                Ok(())
            },
            ResultStatus::Finished => {
                writeln!(f, "Finished without a counterexample (passed={})", self.run.passed)?;
                for d in &self.diagnostics {
                    writeln!(f, "\nConfiguration issue: {d}")?;
                }
                Ok(())
            },
            ResultStatus::GaveUp => writeln!(f, "Gave up after too many discards"),
            ResultStatus::TimedOut => writeln!(f, "Timed out"),
            ResultStatus::Aborted { error } => writeln!(f, "Aborted: {error}"),
        }
    }
}

/// Run a profiling property test and locate the bug. On counterexample,
/// computes SBFL metrics + applies the crash-stack prior in-process and
/// returns the ranked suspect list.
///
/// This is the recommended end-user entry point. From a `main()` or a
/// `#[test]`, a single call performs build-coverage-driven fault
/// localization. See the `quickcheck_with_locate!` macro in the crate root
/// for a form that auto-supplies `module = env!("CARGO_CRATE_NAME")`.
///
/// # Workdir / `LLVM_PROFILE_FILE`
///
/// Each call gets its own subdirectory under `coverage/` (parallelism-safe
/// for `cargo test`). If `LLVM_PROFILE_FILE` is not set, this function sets
/// it to `<workdir>/snapshot_%p-%m.profraw` so subsequent instrumentation
/// writes land in the right place. Callers that have already configured
/// `LLVM_PROFILE_FILE` themselves (e.g. the etna-ify benchmark workflow)
/// are not overridden.
pub fn quickcheck_with_locate<T: Arbitrary<ThreadRng> + Mutate<ThreadRng> + Clone + Debug>(
    f: fn(T) -> Option<bool>,
    module: &str,
) -> LocateResult {
    // Serialize all calls within this process. The LLVM coverage runtime
    // writes profraws to a single process-global filename pattern, so two
    // parallel `cargo test` threads would otherwise race on the rename
    // glob inside `snapshot()` and corrupt each other's data. We allocate
    // distinct per-call workdirs so the *files* don't collide; this mutex
    // ensures the *runtime state* (set_filename, panic hook) doesn't
    // either. From the user's perspective parallel `cargo test` still
    // works — `quickcheck_with_locate` calls just queue.
    static SERIALIZE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = SERIALIZE.lock().unwrap_or_else(|e| e.into_inner());

    let workdir = allocate_workdir();
    let _ = fs::create_dir_all(&workdir);

    // Route subsequent profraw writes into the per-call workdir. We always
    // override here — the env var that may have been set at process start
    // would otherwise cache and ignore later changes. Set both for
    // belt-and-braces (some toolchains read the env var lazily).
    let pattern = workdir.join("snapshot_%p-%m.profraw");
    if let Some(s) = pattern.to_str() {
        set_profile_filename(s);
    }
    std::env::set_var("LLVM_PROFILE_FILE", &pattern);

    let run = quickcheck_in(&workdir, f);
    let mut diagnostics: Vec<analyze::SetupDiagnostic> = Vec::new();

    if !matches!(run.status, ResultStatus::Failed { .. }) {
        diagnostics.push(analyze::SetupDiagnostic::NoCounterexample { iters: run.passed });
        return LocateResult {
            run,
            suspects: vec![],
            n_panics: 0,
            diagnostics,
            error: None,
            workdir,
        };
    }

    let bin = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            return LocateResult {
                run,
                suspects: vec![],
                n_panics: 0,
                diagnostics,
                error: Some(format!("cannot resolve current_exe: {e}")),
                workdir,
            };
        },
    };

    let opts = analyze::AnalyzeOptions {
        coverage_dir: &workdir,
        profdata_dir: None,
        binary_path: &bin,
        module,
        strict_module_filter: true,
    };

    match analyze::analyze_coverage(&opts) {
        Ok((regions, diag)) => {
            // Translate analyzer-level diagnostics into user-facing ones.
            if diag.functions_total == 0 {
                diagnostics.push(analyze::SetupDiagnostic::NoCoverageData);
            } else if diag.functions_after_filter == 0 {
                diagnostics.push(analyze::SetupDiagnostic::ModuleFilterMatchedNothing {
                    module: module.to_string(),
                    candidates: diag.crate_prefixes_seen.clone(),
                });
            }
            let panics = analyze::load_panic_locations(&workdir);
            let n_panics = std::fs::read_to_string(workdir.join("panic_locations.jsonl"))
                .map(|s| s.lines().count())
                .unwrap_or(0);
            let suspects = analyze::rank_with_prior(regions, &panics);
            LocateResult { run, suspects, n_panics, diagnostics, error: None, workdir }
        },
        Err(e) => {
            // Map specific analyzer errors to user-facing diagnostics.
            match &e {
                analyze::AnalyzeError::ToolchainSpawn(detail) => {
                    let tool = if detail.contains("llvm-profdata") {
                        "llvm-profdata"
                    } else if detail.contains("llvm-cov") {
                        "llvm-cov"
                    } else {
                        "llvm tool"
                    };
                    diagnostics.push(analyze::SetupDiagnostic::LlvmToolMissing {
                        tool: tool.to_string(),
                        detail: detail.clone(),
                    });
                },
                analyze::AnalyzeError::MissingIndices(_) => {
                    // status was Failed but we have no indices — runner produced
                    // no snapshots, almost always because the binary lacks
                    // instrumentation.
                    diagnostics.push(analyze::SetupDiagnostic::NoCoverageData);
                },
                _ => {},
            }
            LocateResult {
                run,
                suspects: vec![],
                n_panics: 0,
                diagnostics,
                error: Some(e.to_string()),
                workdir,
            }
        },
    }
}
