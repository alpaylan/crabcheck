//! Library-callable SBFL analysis. The `crabcheck-profiling-fast-analyze`
//! binary is now a thin CLI wrapper over [`analyze_coverage`] +
//! [`load_panic_locations`] + [`rank_with_prior`].
//!
//! Public surface:
//! - [`AnalyzeOptions`] — caller's coverage layout
//! - [`Region`] — per-region SBFL metrics
//! - [`PanicLocation`] — frame parsed from `coverage/panic_locations.jsonl`
//! - [`RankedRegion`] / [`ConfidenceBand`] — output of [`rank_with_prior`]
//! - [`AnalyzeError`] — distinguished error variants

#![cfg(feature = "profiling")]

use {
    crate::sbfl::Suspiciousness,
    rayon::prelude::*,
    rustc_demangle::demangle,
    serde::Deserialize,
    serde_json::Value,
    std::{
        collections::HashMap,
        env,
        fs::File,
        io::{
            BufRead,
            BufReader,
        },
        path::{
            Path,
            PathBuf,
        },
        process::{
            Command,
            Stdio,
        },
        sync::Mutex,
    },
};

/// Inputs for [`analyze_coverage`].
pub struct AnalyzeOptions<'a> {
    /// Directory containing `snapshot_iteration_*.profraw` and `indices.json`
    /// (and optionally `panic_locations.jsonl`). The runner writes here.
    pub coverage_dir: &'a Path,
    /// Where to find / create the per-snapshot `.profdata` files. Defaults to
    /// `coverage_dir.parent()/profdata` if `None`.
    pub profdata_dir: Option<&'a Path>,
    /// Path to the instrumented binary that produced the coverage data.
    /// `llvm-cov export` reads coverage mapping from this binary.
    pub binary_path: &'a Path,
    /// Substring filter for function names — keep only those whose mangled
    /// name contains this string. Typically the consumer's crate name.
    pub module: &'a str,
    /// If true, also require the *demangled* function name to start with
    /// `<module>::` (after stripping a leading `<` for trait impls). This
    /// excludes the false positives that the bare mangled-substring check
    /// admits — Rust's v0 mangling encodes the consuming crate's name into
    /// every monomorphized symbol, so `contains("locate_demo")` matches
    /// `<u32 as serde_core::ser::Serialize>::serialize` when serialized
    /// inside a `locate_demo` binary. The strict filter is the right
    /// default for the user-facing API; the legacy CLI keeps it off so its
    /// `--print-json` output stays byte-identical for the etna-ify
    /// benchmark's existing v2 matrix data.
    pub strict_module_filter: bool,
}

/// Per-(file, function, region) SBFL metrics. The output of
/// [`analyze_coverage`] is a `Vec<Region>` flat across the binary.
#[derive(Clone, Debug)]
pub struct Region {
    pub file: String,
    pub function: String,
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
    pub ef: u64,
    pub ep: u64,
    pub nf: u64,
    pub np: u64,
    pub positive_avg: f64,
    pub negative_avg: f64,
    pub delta: f64,
    pub suspiciousness: Suspiciousness,
}

/// Distinguished failure modes for [`analyze_coverage`].
#[derive(Debug)]
pub enum AnalyzeError {
    /// `coverage/indices.json` was missing or unparseable.
    MissingIndices(String),
    /// `llvm-profdata merge` failed for one snapshot.
    ProfdataMergeFailed { iter: usize, detail: String },
    /// `llvm-cov export` produced output we couldn't parse.
    LlvmCovParseFailed { iter: usize, detail: String },
    /// Failed to spawn / locate `llvm-cov` or `llvm-profdata`.
    ToolchainSpawn(String),
    /// Some other I/O or filesystem error.
    Io(String),
}

impl std::fmt::Display for AnalyzeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnalyzeError::MissingIndices(s) => write!(f, "indices.json: {s}"),
            AnalyzeError::ProfdataMergeFailed { iter, detail } => {
                write!(f, "llvm-profdata merge failed for iter {iter}: {detail}")
            },
            AnalyzeError::LlvmCovParseFailed { iter, detail } => {
                write!(f, "llvm-cov export parse failed for iter {iter}: {detail}")
            },
            AnalyzeError::ToolchainSpawn(s) => write!(f, "toolchain spawn: {s}"),
            AnalyzeError::Io(s) => write!(f, "io: {s}"),
        }
    }
}

impl std::error::Error for AnalyzeError {}

#[derive(Debug, Clone, Hash, Eq, PartialEq, PartialOrd, Ord)]
struct RegionKey {
    fname: String,
    func: String,
    sl: u32,
    sc: u32,
    el: u32,
    ec: u32,
}

#[derive(Deserialize)]
struct Indices {
    positives: Vec<usize>,
    negatives: Vec<usize>,
}

// Minimal deserialization of `llvm-cov export --format=text`:
//   { "data": [{ "functions": [{ "name": ..., "filenames": [...],
//                                 "regions": [[sl,sc,el,ec,count,...], ...] }] }] }
#[derive(Deserialize)]
struct LlvmCovExport {
    data: Vec<LlvmCovData>,
}

#[derive(Deserialize)]
struct LlvmCovData {
    functions: Vec<LlvmCovFunction>,
}

#[derive(Deserialize)]
struct LlvmCovFunction {
    name: String,
    filenames: Vec<String>,
    regions: Vec<Vec<Value>>,
}

#[derive(Default, Clone)]
struct Counts {
    ef: u64,        // failing snapshots where region was hit (count > 0)
    ep: u64,        // passing snapshots where region was hit
    neg_count: u64, // sum of counts over failing snapshots
    pos_count: u64, // sum of counts over passing snapshots
}

fn region_u64(region: &[Value], i: usize) -> u64 {
    region
        .get(i)
        .and_then(|v| v.as_u64().or_else(|| v.as_i64().map(|x| x.max(0) as u64)))
        .unwrap_or(0)
}

fn region_u32(region: &[Value], i: usize) -> u32 {
    region_u64(region, i) as u32
}

/// Return the absolute path to rustc's llvm-tools bin directory so we can
/// find `llvm-profdata` / `llvm-cov` without relying on the caller's PATH.
fn rustc_llvm_bin() -> Option<String> {
    let sysroot = Command::new("rustc").arg("--print").arg("sysroot").output().ok()?;
    if !sysroot.status.success() {
        return None;
    }
    let sysroot_path = String::from_utf8(sysroot.stdout).ok()?.trim().to_string();

    let vv = Command::new("rustc").arg("-vV").output().ok()?;
    if !vv.status.success() {
        return None;
    }
    let host = String::from_utf8(vv.stdout)
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("host: ").map(|s| s.trim().to_string()))?;

    Some(format!("{sysroot_path}/lib/rustlib/{host}/bin"))
}

/// Side-channel data from [`analyze_coverage`] used by the orchestrator to
/// surface configuration issues to the user. Unused by the CLI binary.
#[derive(Default, Debug, Clone)]
pub struct AnalyzeDiagnostics {
    /// Number of distinct functions seen across all snapshots, before any
    /// filter. 0 means the binary has no instrumentation regions or no
    /// snapshots were generated.
    pub functions_total: usize,
    /// Number of functions kept after the `module` substring + (optional)
    /// strict-prefix filter. 0 here is the canonical "wrong module name"
    /// signal: the binary HAD code, but none of it matches.
    pub functions_after_filter: usize,
    /// Top distinct `<crate>::` prefixes (after `<` strip + first `::`) seen
    /// across the binary, sorted by descending function count. The
    /// orchestrator surfaces these as candidate module names when
    /// `functions_after_filter == 0`.
    pub crate_prefixes_seen: Vec<(String, usize)>,
}

/// Run the per-snapshot SBFL pipeline: lazily merge profraw → profdata,
/// invoke `llvm-cov export` per snapshot, accumulate per-region hit counts,
/// and compute SBFL metrics. Returns `(Vec<Region>, AnalyzeDiagnostics)` —
/// the regions are sorted by `(file, start_line, start_col, end_line,
/// end_col, function)`; the diagnostics describe what the filter saw vs.
/// what it kept, useful for surfacing setup issues to the user.
///
/// Spawns up to N processes in parallel via rayon.
pub fn analyze_coverage(
    opts: &AnalyzeOptions<'_>,
) -> Result<(Vec<Region>, AnalyzeDiagnostics), AnalyzeError> {
    let augmented_path = match rustc_llvm_bin() {
        Some(bin) => {
            let existing = env::var("PATH").unwrap_or_default();
            if existing.is_empty() { bin } else { format!("{bin}:{existing}") }
        },
        None => env::var("PATH").unwrap_or_default(),
    };

    let indices_path = opts.coverage_dir.join("indices.json");
    let indices_file = File::open(&indices_path)
        .map_err(|e| AnalyzeError::MissingIndices(format!("{}: {e}", indices_path.display())))?;
    let indices: Indices = serde_json::from_reader(BufReader::new(indices_file))
        .map_err(|e| AnalyzeError::MissingIndices(format!("parse: {e}")))?;
    let positive_samples = indices.positives.len();
    let negative_samples = indices.negatives.len();

    let profdata_dir: PathBuf = match opts.profdata_dir {
        Some(p) => p.to_path_buf(),
        None => {
            let parent = opts.coverage_dir.parent().unwrap_or_else(|| Path::new("."));
            parent.join("profdata")
        },
    };
    std::fs::create_dir_all(&profdata_dir)
        .map_err(|e| AnalyzeError::Io(format!("create {}: {e}", profdata_dir.display())))?;

    let mut snapshots: Vec<(usize, bool)> = Vec::with_capacity(positive_samples + negative_samples);
    snapshots.extend(indices.positives.iter().map(|i| (*i, false)));
    snapshots.extend(indices.negatives.iter().map(|i| (*i, true)));

    let accum: Mutex<HashMap<RegionKey, Counts>> = Mutex::new(HashMap::new());
    // Track distinct functions seen / kept and `<crate>::` prefix counts
    // across all snapshots. We only insert into these on the FIRST snapshot
    // we see a given function in, so the totals reflect "distinct functions"
    // not "function-snapshot pairs".
    let seen_funcs: Mutex<std::collections::HashSet<String>> = Mutex::new(Default::default());
    let kept_funcs: Mutex<std::collections::HashSet<String>> = Mutex::new(Default::default());
    let prefix_counts: Mutex<HashMap<String, usize>> = Mutex::new(HashMap::new());

    snapshots.par_iter().try_for_each(|(idx, is_neg)| -> Result<(), AnalyzeError> {
        let profdata_path = profdata_dir.join(format!("snapshot_iteration_{}.profdata", idx));

        if !profdata_path.exists() {
            let profraw_path =
                opts.coverage_dir.join(format!("snapshot_iteration_{}.profraw", idx));
            if !profraw_path.exists() {
                return Ok(());
            }
            let status = Command::new("llvm-profdata")
                .arg("merge")
                .arg("-sparse")
                .arg(&profraw_path)
                .arg("-o")
                .arg(&profdata_path)
                .env("PATH", &augmented_path)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map_err(|e| {
                    AnalyzeError::ToolchainSpawn(format!("llvm-profdata (iter {idx}): {e}"))
                })?;
            if !status.success() {
                return Err(AnalyzeError::ProfdataMergeFailed {
                    iter: *idx,
                    detail: format!("exit {}", status.code().unwrap_or(-1)),
                });
            }
        }

        let child = Command::new("llvm-cov")
            .arg("export")
            .arg(opts.binary_path)
            .arg("--instr-profile")
            .arg(&profdata_path)
            .arg("--format=text")
            .arg("--skip-expansions")
            .env("PATH", &augmented_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| AnalyzeError::ToolchainSpawn(format!("llvm-cov: {e}")))?;

        let stdout =
            child.stdout.ok_or_else(|| AnalyzeError::ToolchainSpawn("no stdout".into()))?;
        let export: LlvmCovExport = serde_json::from_reader(BufReader::new(stdout))
            .map_err(|e| AnalyzeError::LlvmCovParseFailed { iter: *idx, detail: e.to_string() })?;

        let mut local: HashMap<RegionKey, u64> = HashMap::new();
        for data in export.data {
            for func in data.functions {
                // Track distinct-function diagnostics regardless of filter.
                let was_new = {
                    let mut s =
                        seen_funcs.lock().map_err(|_| AnalyzeError::Io("poisoned lock".into()))?;
                    s.insert(func.name.clone())
                };
                if was_new {
                    // Pull a `<crate>::` prefix out of the demangled name.
                    let demangled = format!("{:#}", demangle(&func.name));
                    let trimmed = demangled.trim_start_matches('<');
                    if let Some(end) = trimmed.find("::") {
                        let prefix = trimmed[..end].to_string();
                        let mut c = prefix_counts
                            .lock()
                            .map_err(|_| AnalyzeError::Io("poisoned lock".into()))?;
                        *c.entry(prefix).or_insert(0) += 1;
                    }
                }

                if !func.name.contains(opts.module) {
                    continue;
                }
                if opts.strict_module_filter {
                    let demangled = format!("{:#}", demangle(&func.name));
                    let trimmed = demangled.trim_start_matches('<');
                    let prefix = format!("{}::", opts.module);
                    if !trimmed.starts_with(&prefix) {
                        continue;
                    }
                }
                if was_new {
                    let mut k =
                        kept_funcs.lock().map_err(|_| AnalyzeError::Io("poisoned lock".into()))?;
                    k.insert(func.name.clone());
                }
                let fname = func.filenames.first().cloned().unwrap_or_default();
                for region in &func.regions {
                    if region.len() < 5 {
                        continue;
                    }
                    let key = RegionKey {
                        fname: fname.clone(),
                        func: func.name.clone(),
                        sl: region_u32(region, 0),
                        sc: region_u32(region, 1),
                        el: region_u32(region, 2),
                        ec: region_u32(region, 3),
                    };
                    *local.entry(key).or_insert(0) += region_u64(region, 4);
                }
            }
        }

        let mut global = accum.lock().map_err(|_| AnalyzeError::Io("poisoned lock".into()))?;
        for (key, count) in local {
            let entry = global.entry(key).or_default();
            if *is_neg {
                if count > 0 {
                    entry.ef += 1;
                }
                entry.neg_count += count;
            } else {
                if count > 0 {
                    entry.ep += 1;
                }
                entry.pos_count += count;
            }
        }
        Ok(())
    })?;

    let global = accum.into_inner().map_err(|_| AnalyzeError::Io("poisoned lock".into()))?;

    let pos_len = (positive_samples as f64).max(1.0);
    let neg_len = (negative_samples as f64).max(1.0);
    let total_pos = positive_samples as u64;
    let total_neg = negative_samples as u64;

    let mut regions: Vec<Region> = global
        .into_iter()
        .map(|(key, c)| {
            let positive_avg = c.pos_count as f64 / pos_len;
            let negative_avg = c.neg_count as f64 / neg_len;
            let delta = negative_avg - positive_avg;
            let nf = total_neg.saturating_sub(c.ef);
            let np = total_pos.saturating_sub(c.ep);
            let suspiciousness = Suspiciousness::compute(c.ef, c.ep, nf, np);
            Region {
                file: key.fname,
                function: format!("{:#}", demangle(&key.func)),
                start_line: key.sl,
                start_col: key.sc,
                end_line: key.el,
                end_col: key.ec,
                ef: c.ef,
                ep: c.ep,
                nf,
                np,
                positive_avg,
                negative_avg,
                delta,
                suspiciousness,
            }
        })
        .collect();

    regions.sort_by(|a, b| {
        (&a.file, a.start_line, a.start_col, a.end_line, a.end_col, &a.function).cmp(&(
            &b.file,
            b.start_line,
            b.start_col,
            b.end_line,
            b.end_col,
            &b.function,
        ))
    });

    let functions_total =
        seen_funcs.into_inner().map_err(|_| AnalyzeError::Io("poisoned lock".into()))?.len();
    let functions_after_filter =
        kept_funcs.into_inner().map_err(|_| AnalyzeError::Io("poisoned lock".into()))?.len();
    let mut crate_prefixes_seen: Vec<(String, usize)> = prefix_counts
        .into_inner()
        .map_err(|_| AnalyzeError::Io("poisoned lock".into()))?
        .into_iter()
        .collect();
    crate_prefixes_seen.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    crate_prefixes_seen.truncate(10);

    let diag = AnalyzeDiagnostics { functions_total, functions_after_filter, crate_prefixes_seen };
    Ok((regions, diag))
}

/// Return `(positive_samples, negative_samples)` from `<coverage>/indices.json`.
/// Cheap helper — used by the CLI binary alongside [`analyze_coverage`] for
/// the top-level JSON envelope.
pub fn read_sample_counts(coverage_dir: &Path) -> Result<(usize, usize), AnalyzeError> {
    let path = coverage_dir.join("indices.json");
    let f = File::open(&path)
        .map_err(|e| AnalyzeError::MissingIndices(format!("{}: {e}", path.display())))?;
    let idx: Indices = serde_json::from_reader(BufReader::new(f))
        .map_err(|e| AnalyzeError::MissingIndices(format!("parse: {e}")))?;
    Ok((idx.positives.len(), idx.negatives.len()))
}

/// One captured panic frame inside the workload's source tree. Both the
/// immediate `info.location()` and frames parsed out of the backtrace are
/// returned; the workload-source-prefix filter drops std/core frames and the
/// `etna-faultloc.rs` / `etna.rs` framework files.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct PanicLocation {
    pub file: String,
    pub line: u32,
}

#[derive(Deserialize)]
struct PanicJsonl {
    file: String,
    line: u32,
    #[serde(default)]
    bt: String,
}

/// Read `coverage/panic_locations.jsonl` (one JSON object per line) and
/// return the deduplicated set of panic locations: the immediate
/// `info.location()` + every backtrace frame matching `./src/*` or `src/*`,
/// minus `src/bin/...` and `src/etna.rs` framework noise.
pub fn load_panic_locations(coverage_dir: &Path) -> Vec<PanicLocation> {
    let path = coverage_dir.join("panic_locations.jsonl");
    let Ok(file) = File::open(&path) else { return Vec::new() };
    let mut out: std::collections::BTreeSet<PanicLocation> = Default::default();
    let bt_re = regex_lite_at();
    for line in BufReader::new(file).lines().flatten() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<PanicJsonl>(trimmed) else { continue };
        if !entry.file.is_empty() {
            out.insert(PanicLocation { file: entry.file.clone(), line: entry.line });
        }
        for bt_line in entry.bt.split('\n') {
            if let Some((file, line)) = bt_re(bt_line) {
                let norm = if let Some(rest) = file.strip_prefix("./") { rest } else { &file[..] };
                let is_workload = norm.starts_with("src/")
                    && !norm.starts_with("src/bin/")
                    && norm != "src/etna.rs";
                if is_workload {
                    out.insert(PanicLocation { file: norm.to_string(), line });
                }
            }
        }
    }
    out.into_iter().collect()
}

/// Tiny dependency-free `at <path>:<line>:<col>` extractor — returns
/// (path, line) on match. Avoids pulling in the `regex` crate.
fn regex_lite_at() -> impl Fn(&str) -> Option<(String, u32)> {
    |s: &str| {
        let s = s.trim_start();
        let after = s.strip_prefix("at ")?;
        let mut parts = after.splitn(3, ':');
        let path = parts.next()?;
        let line_str = parts.next()?;
        // The third part is the column; we don't need it but require it to be
        // present so we don't accidentally match `at file:line` without a col
        // (which doesn't appear in Rust's backtrace format).
        let _col = parts.next()?;
        let line: u32 = line_str.parse().ok()?;
        if path.contains(char::is_whitespace) {
            return None;
        }
        Some((path.to_string(), line))
    }
}

impl PartialOrd for PanicLocation {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for PanicLocation {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (&self.file, self.line).cmp(&(&other.file, other.line))
    }
}

/// User-facing setup / configuration diagnostic. Surfaces specific failure
/// modes with a one-line fix hint instead of opaque error messages.
#[derive(Clone, Debug)]
pub enum SetupDiagnostic {
    /// `analyze_coverage` returned no regions — typically because the binary
    /// wasn't built with `-C instrument-coverage` (so no `__llvm_profile_*`
    /// runtime ran), or because the runner produced no profraw files.
    NoCoverageData,
    /// `llvm-cov` or `llvm-profdata` not found / failed to spawn. The
    /// orchestrator augments PATH from `rustc --print sysroot` already, so
    /// this typically means `llvm-tools-preview` isn't installed.
    LlvmToolMissing { tool: String, detail: String },
    /// The module substring filter excluded every function in the binary.
    /// Almost always a wrong module name (e.g. integration tests calling
    /// the 1-arg macro form, where `CARGO_CRATE_NAME` is the test file's
    /// stem rather than the package name). Includes a sample of crate
    /// prefixes the binary actually contains so the user can pick.
    ModuleFilterMatchedNothing { module: String, candidates: Vec<(String, usize)> },
    /// The run finished without ever observing a counterexample. Either
    /// the property is correct, or the input range is too narrow for the
    /// available iteration budget.
    NoCounterexample { iters: u64 },
}

impl SetupDiagnostic {
    /// Multi-line user-facing hint: what likely went wrong + what to do.
    pub fn hint(&self) -> String {
        match self {
            SetupDiagnostic::NoCoverageData => {
                "No coverage data was produced. Make sure your binary is built \
                 with coverage instrumentation:\n  \
                 [build]\n  \
                 rustflags = [\"-C\", \"instrument-coverage\", \"-C\", \"link-dead-code\", \
                 \"-C\", \"codegen-units=1\", \"-C\", \"debuginfo=2\"]\n\
                 in your `.cargo/config.toml`. See \
                 `crabcheck/examples/locate-demo/.cargo/config.toml`."
                    .into()
            },
            SetupDiagnostic::LlvmToolMissing { tool, detail } => {
                format!(
                    "Couldn't run `{tool}`: {detail}\nTry: `rustup component add llvm-tools-preview` \
                     (ships with the toolchain) or `brew install llvm` (system-wide)."
                )
            },
            SetupDiagnostic::ModuleFilterMatchedNothing { module, candidates } => {
                let cand = if candidates.is_empty() {
                    "  (no candidates: binary appears to have no instrumented code)".into()
                } else {
                    candidates
                        .iter()
                        .map(|(name, n)| format!("  - `{name}::*`  ({n} functions)"))
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                format!(
                    "Module `{module}` matched zero functions in the binary. This usually means \
                     either the module name is wrong, or you're calling the 1-arg \
                     `quickcheck_with_locate!` macro from a `tests/` integration test (where \
                     `CARGO_CRATE_NAME` is the test file's stem, not the package name).\n\n\
                     Crate prefixes the binary actually contains:\n{cand}\n\n\
                     If one of these is your library, switch to the 2-arg macro form:\n  \
                     crabcheck_locate::quickcheck_with_locate!(your_property, \"<crate>\");"
                )
            },
            SetupDiagnostic::NoCounterexample { iters } => {
                format!(
                    "Property held for all {iters} iterations — no counterexample to localize. \
                     If you expected the property to fail: widen `Arbitrary::generate`'s input \
                     range, or set `CRABCHECK_PROFILING_RANDOM_ITERS=10000` (or higher) and rerun."
                )
            },
        }
    }

    /// Short tag for log-friendly contexts (single line, no detail).
    pub fn tag(&self) -> &'static str {
        match self {
            SetupDiagnostic::NoCoverageData => "no_coverage_data",
            SetupDiagnostic::LlvmToolMissing { .. } => "llvm_tool_missing",
            SetupDiagnostic::ModuleFilterMatchedNothing { .. } => "module_filter_matched_nothing",
            SetupDiagnostic::NoCounterexample { .. } => "no_counterexample",
        }
    }
}

impl std::fmt::Display for SetupDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}]\n{}", self.tag(), self.hint())
    }
}

/// Confidence band for [`RankedRegion`]. Calibrated against the v2 matrix
/// (see `scripts/aggregate_ranks_v2.py`):
///
/// - `High` — singleton panic-overlap tier OR both ochiai and delta rank truth #1
/// - `Medium` — exactly one of (ochiai, delta) ranks truth #1
/// - `MediumLow` — panic data exists but multi-tier (>1 panic-overlap groups)
/// - `LowMedium` — default (no strong signal either way)
/// - `Low` — ochiai and delta disagree by more than 5 ranks
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfidenceBand {
    High,
    Medium,
    MediumLow,
    LowMedium,
    Low,
}

impl std::fmt::Display for ConfidenceBand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ConfidenceBand::High => "HIGH",
            ConfidenceBand::Medium => "MEDIUM",
            ConfidenceBand::MediumLow => "MEDIUM_LOW",
            ConfidenceBand::LowMedium => "LOW_MEDIUM",
            ConfidenceBand::Low => "LOW",
        };
        f.write_str(s)
    }
}

/// One ranked function group post-prior. Multiple regions of the same
/// function are merged: line range widens to cover all constituent regions,
/// metric values are taken as the max across constituents.
#[derive(Clone, Debug)]
pub struct RankedRegion {
    pub region: Region,
    pub avg_norm_od: f64,
    pub panic_overlap: bool,
    /// 1-based.
    pub rank: usize,
    pub confidence: ConfidenceBand,
    pub confidence_rule: &'static str,
}

/// Function-level ranking with the crash-stack prior. Mirrors the logic in
/// `scripts/aggregate_ranks_v2.py:per_config_ranks` + `confidence_band`:
///
/// 1. Group regions by `(file, function)`; widen line range, take the max
///    ochiai/delta/etc. across constituents.
/// 2. Per-variant max-normalize ochiai and delta; `avg_norm_od` is their mean.
/// 3. A group has `panic_overlap = true` if any constituent region's range
///    contains any panic location with a matching file path.
/// 4. Sort lex `(panic_overlap desc, avg_norm_od desc)`; assign 1-based rank.
/// 5. Assign each ranked group a `ConfidenceBand` per the calibrated rules.
pub fn rank_with_prior(regions: Vec<Region>, panics: &[PanicLocation]) -> Vec<RankedRegion> {
    if regions.is_empty() {
        return Vec::new();
    }

    let mut groups: HashMap<(String, String), Region> = HashMap::new();
    for r in regions {
        let key = (r.file.clone(), r.function.clone());
        match groups.get_mut(&key) {
            Some(g) => {
                if r.suspiciousness.ochiai > g.suspiciousness.ochiai {
                    g.suspiciousness.ochiai = r.suspiciousness.ochiai;
                }
                if r.suspiciousness.tarantula > g.suspiciousness.tarantula {
                    g.suspiciousness.tarantula = r.suspiciousness.tarantula;
                }
                if r.suspiciousness.dstar > g.suspiciousness.dstar {
                    g.suspiciousness.dstar = r.suspiciousness.dstar;
                }
                if r.suspiciousness.jaccard > g.suspiciousness.jaccard {
                    g.suspiciousness.jaccard = r.suspiciousness.jaccard;
                }
                if r.suspiciousness.op2 > g.suspiciousness.op2 {
                    g.suspiciousness.op2 = r.suspiciousness.op2;
                }
                if r.delta > g.delta {
                    g.delta = r.delta;
                }
                if r.start_line < g.start_line {
                    g.start_line = r.start_line;
                }
                if r.end_line > g.end_line {
                    g.end_line = r.end_line;
                }
            },
            None => {
                groups.insert(key, r);
            },
        }
    }

    let mut grouped: Vec<Region> = groups.into_values().collect();
    grouped.sort_by(|a, b| {
        (&a.file, a.start_line, &a.function).cmp(&(&b.file, b.start_line, &b.function))
    });

    let max_o = grouped.iter().map(|r| r.suspiciousness.ochiai).fold(0.0f64, f64::max);
    let max_d = grouped.iter().map(|r| r.delta).fold(0.0f64, f64::max);

    fn region_overlaps_panic(r: &Region, panics: &[PanicLocation]) -> bool {
        for p in panics {
            if !r.file.ends_with(&p.file) {
                continue;
            }
            if r.start_line <= p.line && p.line <= r.end_line {
                return true;
            }
        }
        false
    }

    let mut staged: Vec<(Region, f64, bool)> = grouped
        .into_iter()
        .map(|r| {
            let n_o = if max_o > 0.0 { r.suspiciousness.ochiai / max_o } else { 0.0 };
            let n_d = if max_d > 0.0 { r.delta.max(0.0) / max_d } else { 0.0 };
            let avg = (n_o + n_d) / 2.0;
            let overlap = region_overlaps_panic(&r, panics);
            (r, avg, overlap)
        })
        .collect();

    staged.sort_by(|a, b| {
        match b.2.cmp(&a.2) {
            std::cmp::Ordering::Equal => b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal),
            o => o,
        }
    });

    let n_panic_overlap_groups: usize = staged.iter().filter(|s| s.2).count();
    let n_panics_total = panics.len();

    let ochiai_rank_1: Option<usize> =
        staged.iter().position(|s| s.0.suspiciousness.ochiai > 0.0).map(|_| 0);
    // Compute per-method ranks of the *first* region the user would care about.
    // Easier: precompute ochiai-rank and delta-rank arrays, find rank of each region.
    let mut ochiai_order: Vec<usize> = (0..staged.len()).collect();
    ochiai_order.sort_by(|&a, &b| {
        staged[b]
            .0
            .suspiciousness
            .ochiai
            .partial_cmp(&staged[a].0.suspiciousness.ochiai)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut ochiai_rank_of = vec![0usize; staged.len()];
    for (rank, idx) in ochiai_order.iter().enumerate() {
        ochiai_rank_of[*idx] = rank + 1;
    }

    let mut delta_order: Vec<usize> = (0..staged.len()).collect();
    delta_order.sort_by(|&a, &b| {
        staged[b].0.delta.partial_cmp(&staged[a].0.delta).unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut delta_rank_of = vec![0usize; staged.len()];
    for (rank, idx) in delta_order.iter().enumerate() {
        delta_rank_of[*idx] = rank + 1;
    }
    let _ = ochiai_rank_1; // silence unused-warning; was a sanity check

    // Threshold for "metric actually discriminates". Without this gate the
    // metric-rank-1 rules trivially fire when *every* region is tied at 0
    // (low-iteration-budget case), which produced 24/29 false-positive HIGH
    // bands at cargo-test default settings. ochiai > 0 means the region
    // was hit only by failing snapshots; delta > 0 means count-mean diff
    // favors failing. Either alone is enough signal to trust rank-1.
    let has_metric_signal = |idx: usize, region: &Region| -> bool {
        let _ = idx;
        region.suspiciousness.ochiai > 0.0 || region.delta > 0.0
    };

    let mut out: Vec<RankedRegion> = Vec::with_capacity(staged.len());
    for (idx, (region, avg_norm_od, panic_overlap)) in staged.into_iter().enumerate() {
        let rank = idx + 1;
        let o_rank = ochiai_rank_of[idx];
        let d_rank = delta_rank_of[idx];
        let signal = has_metric_signal(idx, &region);
        let (band, rule) = match (panic_overlap, n_panic_overlap_groups, o_rank == 1, d_rank == 1) {
            // Crash-stack prior is independent signal — fire HIGH unconditionally.
            (true, 1, _, _) => (ConfidenceBand::High, "singleton_panic_overlap"),
            // Metric-based rules require non-zero signal, otherwise tied-at-zero
            // ranks trivially fire HIGH/MEDIUM at low budgets.
            (_, _, true, true) if signal => (ConfidenceBand::High, "both_metrics_rank_1"),
            (_, _, true, false) if signal => (ConfidenceBand::Medium, "one_metric_rank_1"),
            (_, _, false, true) if signal => (ConfidenceBand::Medium, "one_metric_rank_1"),
            (true, _, _, _) => (ConfidenceBand::MediumLow, "panic_multi_tier"),
            (_, _, _, _) if (o_rank as i64 - d_rank as i64).abs() > 5 && signal => {
                (ConfidenceBand::Low, "metrics_disagree_gt5")
            },
            _ => (ConfidenceBand::LowMedium, "default"),
        };
        let _ = n_panics_total;
        out.push(RankedRegion {
            region,
            avg_norm_od,
            panic_overlap,
            rank,
            confidence: band,
            confidence_rule: rule,
        });
    }
    out
}
