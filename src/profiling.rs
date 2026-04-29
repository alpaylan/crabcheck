use std::{
    fmt::Debug,
    fs,
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
}


pub(crate) fn snapshot(label: &str) {
    tracing::debug!("Taking snapshot: {}", label);
    unsafe {
        __llvm_profile_write_file();
    }

    // Glob to find the current .profraw file
    let files = glob::glob("coverage/*.profraw").unwrap();
    let newest: Option<std::path::PathBuf> = files.filter_map(Result::ok).max_by_key(|path| {
        path.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH)
    });

    if let Some(path) = newest {
        let new_name = format!("coverage/snapshot_{}.profraw", label);
        fs::rename(&path, &new_name).expect("rename failed");
    }
}

pub(crate) fn reset() {
    unsafe { __llvm_profile_reset_counters() };
}

/// Slot shared between the panic hook (writer) and `safe_call` (reader).
/// Holds the most recent panic's (file, line, backtrace) until the wrapped
/// `catch_unwind` returns control to `safe_call`, which drains the slot to
/// `coverage/panic_locations.jsonl`. Single-threaded today, so the Mutex is
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
        let (file, line) = info
            .location()
            .map(|l| (l.file().to_string(), l.line()))
            .unwrap_or_default();
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
/// (populated by the hook installed via `install_panic_capture`) and append a
/// JSONL entry to `coverage/panic_locations.jsonl`. Returns `Some(false)` to
/// the caller so the rest of the loop classifies the iteration as a failure
/// without itself panicking.
fn safe_call<T>(f: fn(T) -> Option<bool>, input: T, slot: &PanicSlot) -> Option<bool> {
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
            if let Ok(mut h) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("coverage/panic_locations.jsonl")
            {
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
/// `quickcheck_with_shrink` with a no-op shrinker.
pub fn quickcheck<T: Arbitrary<ThreadRng> + Mutate<ThreadRng> + Clone + Debug>(
    f: fn(T) -> Option<bool>,
) -> RunResult {
    quickcheck_impl(f, |_| Vec::new())
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
    quickcheck_impl(f, shrink)
}

fn quickcheck_impl<T: Arbitrary<ThreadRng> + Mutate<ThreadRng> + Clone + Debug>(
    f: fn(T) -> Option<bool>,
    shrink: fn(&T) -> Vec<T>,
) -> RunResult {
    let mut rng = rand::rng();
    tracing::debug!("Starting profiling quickcheck...");

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
    // `coverage/panic_locations.jsonl` after a panic; the guard restores the
    // previous hook on drop (i.e. when this function returns).
    let slot: PanicSlot = std::sync::Arc::new(std::sync::Mutex::new(None));
    let _hook_guard = install_panic_capture(slot.clone());

    let mut passed = 0;
    let mut discarded = 0;
    let mut initial_passes: Vec<(usize, String)> = Vec::with_capacity(max_initial_pass_snapshots);
    for i in 0..n {
        let input = T::generate(&mut rng, ((i + 1) as f32).log2() as usize);
        tracing::trace!("Test #{}: {:?}", i, input);
        match safe_call(f, input.clone(), &slot) {
            None => discarded += 1,
            Some(true) => {
                passed += 1;
                if initial_passes.len() < max_initial_pass_snapshots {
                    let label_idx = initial_pass_base + initial_passes.len();
                    crate::profiling::reset();
                    let _ = safe_call(f, input.clone(), &slot);
                    crate::profiling::snapshot(format!("iteration_{label_idx}").as_str());
                    initial_passes.push((label_idx, format!("{:?}", input)));
                }
            },
            Some(false) => {
                // Shrink the failing seed to its minimal still-failing form.
                // Greedy: repeatedly take the first smaller candidate that
                // still fails. Terminates when no candidate fails (local
                // minimum) or the step cap is hit.
                let mut seed = input.clone();
                let mut shrink_steps = 0usize;
                'shrink: loop {
                    let candidates = shrink(&seed);
                    if candidates.is_empty() {
                        break 'shrink;
                    }
                    let mut found_smaller = false;
                    for candidate in candidates {
                        if let Some(false) = safe_call(f, candidate.clone(), &slot) {
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
                    tracing::info!(
                        "shrunk failing input in {shrink_steps} step(s): {:?}",
                        seed
                    );
                }

                let (mut positives, mut negatives) =
                    (initial_passes.clone(), vec![(0, format!("{:?}", seed))]);
                crate::profiling::reset();
                let _ = safe_call(f, seed.clone(), &slot);
                crate::profiling::snapshot(format!("iteration_0").as_str());

                for i in 1..=max_mutations {
                    let input = T::mutate(&seed, &mut rng, ((i + 1) as f32).log2() as usize);
                    crate::profiling::reset();
                    let result = safe_call(f, input.clone(), &slot);
                    crate::profiling::snapshot(format!("iteration_{i}").as_str());
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
                let file_path = format!("coverage/indices.json");
                tracing::debug!("JSON:\n{}\n", json);
                fs::write(file_path, json).expect("Unable to write file");
                tracing::debug!("JSON written to coverage/indices.json");


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
