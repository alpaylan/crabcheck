//! The outcome of a run: [`Report`] and its parts.
//!
//! A report is both human-readable (`Display`) and, with the default `json`
//! feature, machine-readable ([`Report::to_json`]). Everything an author or
//! an agent needs to act on a failure is in it: the seed, the replay point,
//! the failing input before and after shrinking, the panic message and
//! location, timing, and the distribution statistics collected while the
//! property held.

use std::{
    collections::BTreeMap,
    fmt,
    path::PathBuf,
    time::Duration,
};

use crate::config::ReplayPoint;

/// How a run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
pub enum Status {
    /// The required number of tests passed (or the time budget ran out
    /// with every test passing).
    Passed,
    /// A counterexample was found. See [`Report::failure`].
    Failed,
    /// Too many inputs were discarded. See [`Config::max_discard_ratio`](crate::Config::max_discard_ratio).
    GaveUp,
    /// The run exceeded [`Config::timeout_ms`](crate::Config::timeout_ms).
    TimedOut,
    /// The generator itself panicked. See [`Report::abort`].
    Aborted,
}

/// A source location captured from a panic.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
pub struct Location {
    /// File path as reported by the panic.
    pub file: String,
    /// 1-based line.
    pub line: u32,
    /// 1-based column.
    pub column: u32,
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.file, self.line, self.column)
    }
}

/// What kind of failure the property produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
pub enum FailureKind {
    /// The property returned `false`.
    False,
    /// The property returned an error or an explicit failure message.
    Error,
    /// The property panicked.
    Panic,
}

/// One failing input, rendered with `Debug`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
pub struct Counterexample {
    /// `Debug` rendering of the input, possibly truncated.
    pub debug: String,
    /// Whether `debug` was cut short (see `Config::report.truncate_debug_at`).
    pub truncated: bool,
    /// The size parameter the input was generated at.
    pub size: usize,
    /// Zero-based index of the generation attempt that produced it.
    pub iteration: u64,
    /// The worker thread that produced it, for parallel runs.
    pub thread: Option<usize>,
}

/// What shrinking did.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
pub struct ShrinkStats {
    /// Candidates tried.
    pub steps: u64,
    /// Candidates accepted.
    pub accepted: u64,
    /// Candidates skipped because they rendered longer than the current best.
    pub rejected_not_smaller: u64,
    /// Candidates skipped because they had been seen before.
    pub rejected_seen: u64,
    /// Whether the step or time budget stopped shrinking early.
    pub budget_exhausted: bool,
    /// Time spent shrinking.
    pub duration: Duration,
}

/// A counterexample and the failure it triggered.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "json", derive(serde::Serialize))]
pub struct Failure {
    /// What kind of failure.
    pub kind: FailureKind,
    /// The error, failure message, or panic message (of the shrunk input
    /// when shrinking ran).
    pub message: Option<String>,
    /// Where a panic originated, when known.
    pub location: Option<Location>,
    /// The input that first failed.
    pub original: Counterexample,
    /// The shrunk input, when shrinking ran and made progress.
    pub shrunk: Option<Counterexample>,
    /// Where the original input can be regenerated from
    /// (`CRABCHECK_REPLAY=seed:iteration`). `None` when the input came from
    /// the targeted phase; then the whole run must be repeated with
    /// [`Report::seed`].
    pub replay: Option<ReplayPoint>,
    /// Whether the property failed differently when re-run on the same
    /// input. A flaky property cannot be shrunk reliably.
    pub flaky: bool,
    /// What shrinking did, when it ran.
    pub shrink: Option<ShrinkStats>,
}

impl Failure {
    /// The best counterexample available: the shrunk one if any, else the
    /// original.
    pub fn counterexample(&self) -> &Counterexample {
        self.shrunk.as_ref().unwrap_or(&self.original)
    }
}

/// A generator panic.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
pub struct Abort {
    /// The panic message.
    pub message: String,
    /// Where the panic originated, when known.
    pub location: Option<Location>,
    /// Zero-based index of the generation attempt.
    pub iteration: u64,
}

/// Something about the run that an author should look at.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
pub struct HealthWarning {
    /// Stable identifier, e.g. `flaky`, `discard_ratio`, `config_env`.
    pub kind: String,
    /// Human-readable explanation with the numbers behind it.
    pub message: String,
}

/// What happened to the explicit replay point (`CRABCHECK_REPLAY`).
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
pub struct ReplayOutcome {
    /// The point that was replayed.
    pub point: ReplayPoint,
    /// Whether the property failed on it.
    pub reproduced: bool,
}

/// What happened with the regressions file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
pub struct RegressionsSummary {
    /// The file used, when regressions were enabled.
    pub file: Option<PathBuf>,
    /// Stored entries replayed before random testing.
    pub replayed: u64,
    /// Entries whose regenerated input no longer matched the stored one
    /// (the generator changed).
    pub stale: u64,
    /// Whether this run's failure was appended.
    pub saved: bool,
}

/// What the targeted phase achieved (present when the property called
/// [`target`](crate::target)).
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
pub struct TargetSummary {
    /// Mutated inputs evaluated in the targeted phase.
    pub iterations: u64,
    /// Times the best score improved.
    pub improvements: u64,
    /// The highest score observed in the whole run.
    pub best_score: f64,
    /// `Debug` rendering of the input that reached `best_score`.
    pub best_debug: String,
}

/// The outcome of a run of [`check`](crate::check).
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "json", derive(serde::Serialize))]
pub struct Report {
    /// How the run ended.
    pub status: Status,
    /// The seed the run used. Pass it to `Config::seed` to reproduce the
    /// whole run.
    pub seed: u64,
    /// Passing tests.
    pub tests: u64,
    /// Discarded inputs (from generators and from the property).
    pub discarded: u64,
    /// Generation attempts made (passing, discarded and failing).
    pub iterations: u64,
    /// Whether the random phase ended because `Config::budget_ms` ran out
    /// before `Config::tests` inputs had passed.
    pub stopped_by_budget: bool,
    /// The `max_size` the run used.
    pub max_size: usize,
    /// Worker threads the random phase ran on (1 for [`check`](crate::check)).
    pub threads: usize,
    /// Wall-clock time of the whole run.
    pub duration: Duration,
    /// Time spent generating inputs.
    pub gen_time: Duration,
    /// Time spent running the property.
    pub prop_time: Duration,
    /// Share of generated (non-discarded) inputs that were distinct.
    pub distinct_inputs_ratio: f64,
    /// The failure, when `status` is `Failed`.
    pub failure: Option<Failure>,
    /// The generator panic, when `status` is `Aborted`.
    pub abort: Option<Abort>,
    /// What happened to the explicit replay point, if one was given.
    pub replay: Option<ReplayOutcome>,
    /// What happened with the regressions file.
    pub regressions: RegressionsSummary,
    /// The targeted phase, when the property reported scores.
    pub target: Option<TargetSummary>,
    /// Coverage of the crate under test, when collected (`coverage` feature,
    /// instrumented build).
    #[cfg(feature = "coverage")]
    pub coverage: Option<crate::coverage::Summary>,
    /// Fault localization for a failure: the functions ranked by how much
    /// their coverage separates failing runs from passing ones (`coverage`
    /// feature, instrumented build).
    #[cfg(feature = "coverage")]
    pub locate: Option<crate::coverage::Locate>,
    /// Path of the profraw written for `llvm-cov`, when requested.
    pub profraw: Option<PathBuf>,
    /// Labels registered with [`cover`](crate::cover) and the share of passing
    /// tests that carried each.
    pub covers: BTreeMap<String, f64>,
    /// How many passing tests carried each label (see [`label`](crate::label)).
    pub labels: BTreeMap<String, u64>,
    /// How many passing tests ran at each size.
    pub sizes: BTreeMap<usize, u64>,
    /// How many inputs were discarded for each reason.
    pub discard_reasons: BTreeMap<String, u64>,
    /// Things worth looking at.
    pub health: Vec<HealthWarning>,
}

impl Report {
    pub(crate) fn new(seed: u64, max_size: usize) -> Self {
        Report {
            status: Status::Passed,
            seed,
            tests: 0,
            discarded: 0,
            iterations: 0,
            stopped_by_budget: false,
            max_size,
            threads: 1,
            duration: Duration::ZERO,
            gen_time: Duration::ZERO,
            prop_time: Duration::ZERO,
            distinct_inputs_ratio: 1.0,
            failure: None,
            abort: None,
            replay: None,
            regressions: RegressionsSummary::default(),
            target: None,
            #[cfg(feature = "coverage")]
            coverage: None,
            #[cfg(feature = "coverage")]
            locate: None,
            profraw: None,
            covers: BTreeMap::new(),
            labels: BTreeMap::new(),
            sizes: BTreeMap::new(),
            discard_reasons: BTreeMap::new(),
            health: Vec::new(),
        }
    }

    /// Whether the property held.
    pub fn passed(&self) -> bool {
        self.status == Status::Passed
    }

    /// The best counterexample, if the run failed.
    pub fn counterexample(&self) -> Option<&Counterexample> {
        self.failure.as_ref().map(Failure::counterexample)
    }

    /// Panic with the rendered report unless the property held. Use this at
    /// the end of a `#[test]`.
    #[track_caller]
    pub fn assert_passed(&self) {
        if !self.passed() {
            panic!("{self}");
        }
    }

    /// Render the report as pretty-printed JSON.
    #[cfg(feature = "json")]
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("Report is always serialisable")
    }

    pub(crate) fn warn(&mut self, kind: &str, message: impl Into<String>) {
        self.health.push(HealthWarning { kind: kind.to_string(), message: message.into() });
    }
}

fn millis(d: Duration) -> String {
    format!("{:.1} ms", d.as_secs_f64() * 1000.0)
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.status {
            Status::Passed => {
                writeln!(
                    f,
                    "crabcheck: ok, {} tests passed ({} discarded) in {}, seed {}{}",
                    self.tests,
                    self.discarded,
                    millis(self.duration),
                    self.seed,
                    if self.stopped_by_budget { " (stopped by the time budget)" } else { "" }
                )?;
            },
            Status::Failed => {
                let failure = self.failure.as_ref().expect("Failed reports carry a failure");
                let what = match (failure.kind, &failure.message) {
                    (FailureKind::False, _) => "property returned false".to_string(),
                    (FailureKind::Error, Some(m)) => format!("property failed: {m}"),
                    (FailureKind::Error, None) => "property failed".to_string(),
                    (FailureKind::Panic, Some(m)) => format!("property panicked: {m}"),
                    (FailureKind::Panic, None) => "property panicked".to_string(),
                };
                writeln!(f, "crabcheck: FAILED, {what}")?;
                if let Some(location) = &failure.location {
                    writeln!(f, "  at {location}")?;
                }
                let best = failure.counterexample();
                writeln!(
                    f,
                    "  counterexample: {}{}",
                    best.debug,
                    if best.truncated { " (truncated)" } else { "" }
                )?;
                if failure.shrunk.is_some() {
                    writeln!(
                        f,
                        "  original:       {}{} (size {}, iteration {})",
                        failure.original.debug,
                        if failure.original.truncated { " (truncated)" } else { "" },
                        failure.original.size,
                        failure.original.iteration
                    )?;
                } else {
                    writeln!(f, "  found at size {}, iteration {}", best.size, best.iteration)?;
                }
                if let Some(thread) = failure.original.thread {
                    writeln!(
                        f,
                        "  found by worker {thread} of {}; which failure a parallel run reports first can vary",
                        self.threads
                    )?;
                }
                if failure.flaky {
                    writeln!(
                        f,
                        "  FLAKY: the property did not fail the same way when re-run on this input"
                    )?;
                }
                if let Some(shrink) = &failure.shrink {
                    writeln!(
                        f,
                        "  shrinking: {} of {} candidates accepted in {}{}",
                        shrink.accepted,
                        shrink.steps,
                        millis(shrink.duration),
                        if shrink.budget_exhausted { " (budget exhausted)" } else { "" }
                    )?;
                }
                writeln!(
                    f,
                    "  after {} passing tests ({} discarded) in {}",
                    self.tests,
                    self.discarded,
                    millis(self.duration)
                )?;
                match failure.replay {
                    Some(point) => writeln!(f, "  reproduce: CRABCHECK_REPLAY={point}")?,
                    None => {
                        writeln!(
                            f,
                            "  reproduce: CRABCHECK_SEED={} (found in the targeted phase)",
                            self.seed
                        )?
                    },
                }
                writeln!(f, "  or rerun the whole search: Config::default().seed({})", self.seed)?;
                if let Some(file) = &self.regressions.file
                    && self.regressions.saved
                {
                    writeln!(f, "  remembered in {}", file.display())?;
                }
            },
            Status::GaveUp => {
                writeln!(
                    f,
                    "crabcheck: GAVE UP after {} discarded inputs ({} tests passed), seed {}",
                    self.discarded, self.tests, self.seed
                )?;
            },
            Status::TimedOut => {
                writeln!(
                    f,
                    "crabcheck: TIMED OUT after {} ({} tests passed, {} discarded), seed {}",
                    millis(self.duration),
                    self.tests,
                    self.discarded,
                    self.seed
                )?;
            },
            Status::Aborted => {
                let abort = self.abort.as_ref().expect("Aborted reports carry an abort");
                writeln!(f, "crabcheck: ABORTED, the generator panicked: {}", abort.message)?;
                if let Some(location) = &abort.location {
                    writeln!(f, "  at {location}")?;
                }
                writeln!(f, "  iteration {}, seed {}", abort.iteration, self.seed)?;
            },
        }
        if let Some(replay) = &self.replay {
            writeln!(
                f,
                "  replay point {} {}",
                replay.point,
                if replay.reproduced { "reproduced the failure" } else { "passed" }
            )?;
        }
        if self.regressions.replayed > 0 {
            writeln!(
                f,
                "  regressions: {} stored input(s) replayed{}",
                self.regressions.replayed,
                if self.regressions.stale > 0 {
                    format!(", {} stale", self.regressions.stale)
                } else {
                    String::new()
                }
            )?;
        }
        if let Some(target) = &self.target {
            writeln!(
                f,
                "  target: best score {} reached by {} ({} mutated inputs, {} improvements)",
                target.best_score, target.best_debug, target.iterations, target.improvements
            )?;
        }
        #[cfg(feature = "coverage")]
        if let Some(coverage) = &self.coverage {
            writeln!(
                f,
                "  coverage: {} of {} functions{} reached, {} of {} counters",
                coverage.functions_reached,
                coverage.functions_total,
                if coverage.filter.is_empty() {
                    String::new()
                } else {
                    format!(" in `{}`", coverage.filter)
                },
                coverage.counters_hit,
                coverage.counters_total
            )?;
            if !coverage.unreached.is_empty() {
                let shown: Vec<&str> =
                    coverage.unreached.iter().take(5).map(String::as_str).collect();
                let more = coverage.unreached.len().saturating_sub(shown.len());
                writeln!(
                    f,
                    "  never reached: {}{}",
                    shown.join(", "),
                    if more > 0 { format!(" (+{more} more)") } else { String::new() }
                )?;
            }
            if !coverage.partial.is_empty() {
                let shown: Vec<String> = coverage
                    .partial
                    .iter()
                    .take(5)
                    .map(|(name, hit, total)| format!("{name} {hit}/{total}"))
                    .collect();
                let more = coverage.partial.len().saturating_sub(shown.len());
                writeln!(
                    f,
                    "  partly covered: {}{}",
                    shown.join(", "),
                    if more > 0 { format!(" (+{more} more)") } else { String::new() }
                )?;
            }
        }
        if let Some(profraw) = &self.profraw {
            writeln!(f, "  profraw written to {}", profraw.display())?;
        }
        #[cfg(feature = "coverage")]
        if let Some(locate) = &self.locate {
            writeln!(
                f,
                "  suspects ({} passing and {} failing runs over {} regions):",
                locate.passing, locate.failing, locate.regions
            )?;
            for s in &locate.suspects {
                writeln!(
                    f,
                    "    {}. {}:{}-{} in `{}`  ochiai={:.2} delta={:.2}  {}{} ({})",
                    s.rank,
                    crate::coverage::display_path(&s.file),
                    s.start_line,
                    s.end_line,
                    s.function,
                    s.scores.ochiai,
                    s.scores.delta,
                    s.confidence,
                    if s.panic_overlap { " panic-site" } else { "" },
                    s.rule
                )?;
            }
        }
        if !self.covers.is_empty() {
            let covers: Vec<String> = self
                .covers
                .iter()
                .map(|(label, share)| format!("{label} {:.0}%", share * 100.0))
                .collect();
            writeln!(f, "  cover: {}", covers.join(", "))?;
        }
        if !self.discard_reasons.is_empty() {
            let reasons: Vec<String> = self
                .discard_reasons
                .iter()
                .map(|(reason, count)| format!("{reason} x{count}"))
                .collect();
            writeln!(f, "  discards: {}", reasons.join(", "))?;
        }
        if !self.labels.is_empty() && self.tests > 0 {
            let labels: Vec<String> = self
                .labels
                .iter()
                .map(|(label, count)| {
                    format!("{label} {:.0}%", *count as f64 * 100.0 / self.tests as f64)
                })
                .collect();
            writeln!(f, "  labels: {}", labels.join(", "))?;
        }
        for warning in &self.health {
            writeln!(f, "  warning [{}]: {}", warning.kind, warning.message)?;
        }
        Ok(())
    }
}
