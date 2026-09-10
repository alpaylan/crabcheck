//! The test loop: [`check`], [`check_par`] and [`quickcheck`].

pub mod labels;
mod panic;
pub mod testable;

use std::{
    collections::{
        BTreeMap,
        HashSet,
    },
    fmt::{
        self,
        Debug,
        Write as _,
    },
    hash::{
        BuildHasher,
        DefaultHasher,
        Hasher,
        RandomState,
    },
    sync::atomic::{
        AtomicBool,
        Ordering,
    },
    time::Instant,
};

use {
    rand_chacha::ChaCha8Rng,
    rand_core::SeedableRng,
};

use crate::{
    Arbitrary,
    Config,
    Gen,
    Source,
    any,
    config::ReplayPoint,
    health,
    pool::Pool,
    report::{
        Abort,
        Counterexample,
        Failure,
        FailureKind,
        Location,
        ReplayOutcome,
        Report,
        ShrinkStats,
        Status,
        TargetSummary,
    },
    shrink,
    source::{
        u01,
        uniform,
    },
};
pub use testable::{
    Outcome,
    TestResult,
    Testable,
};

use self::panic::PanicInfo;

/// Run `property` against inputs from `generator` until [`Config::tests`]
/// inputs pass, one fails, too many are discarded, or a time limit elapses.
///
/// Before random testing, an explicit replay point ([`Config::replay`] or
/// `CRABCHECK_REPLAY`) and any stored regressions for this property are
/// regenerated and tested. A failing input is shrunk with the generator's
/// shrinker (see [`Config::shrink`]), remembered in the regressions file,
/// and reported with the seed and iteration that regenerate it.
/// Environment overrides (`CRABCHECK_*`, see [`config`](crate::config)) are
/// applied on top of `config`.
///
/// If the property reports scores through [`target`](crate::target), the
/// random phase is followed by a targeted phase that mutates the
/// best-scoring inputs with the generator's mutator (see [`Config::target`]).
///
/// ```
/// use crabcheck::{check, gens, Config};
///
/// let report = check(&Config::default().seed(1).tests(200), gens::vec(gens::int(-100i32..=100)), |mut v| {
///     v.sort();
///     v.windows(2).all(|w| w[0] <= w[1])
/// });
/// report.assert_passed();
/// ```
pub fn check<G, R, P>(config: &Config, generator: G, property: P) -> Report
where
    G: Gen,
    G::Value: Clone + Debug,
    R: Testable,
    P: FnMut(G::Value) -> R,
{
    let mut run = Run::new(config, generator, property);
    if run.remembered() {
        return run.finish();
    }
    run.random_search(None);
    if run.pending.is_none() && run.report.status == Status::Passed {
        run.targeted_search();
    }
    run.finish()
}

/// [`check`] with the type's [`Arbitrary`] generator and the default
/// [`Config`].
///
/// ```
/// use crabcheck::quickcheck;
///
/// quickcheck(|(a, b): (u8, u8)| u16::from(a) + u16::from(b) >= u16::from(a)).assert_passed();
/// ```
pub fn quickcheck<T, R, P>(property: P) -> Report
where
    T: Arbitrary + Clone + Debug,
    R: Testable,
    P: FnMut(T) -> R,
{
    check(&Config::default(), any::<T>(), property)
}

/// [`check`] with the random phase spread over [`Config::threads`] worker
/// threads. Each worker derives its own seed from the run's seed and runs
/// its share of the tests; the first failure any worker finds is shrunk
/// and reported on the calling thread, and the targeted phase (if any)
/// runs there as well. Which failure is reported first can vary between
/// runs; the replay point in the report regenerates it exactly regardless.
///
/// The property must be `Fn + Sync` because workers share it. Replay
/// points and regressions are not consulted here; use [`check`] for those.
///
/// ```
/// use crabcheck::{check_par, gens, Config};
///
/// let report = check_par(&Config::default().threads(4).tests(400), gens::vec(gens::int(0i32..=9)), |v| v.len() <= 100);
/// report.assert_passed();
/// assert_eq!(report.threads, 4);
/// ```
pub fn check_par<G, R, P>(config: &Config, generator: G, property: P) -> Report
where
    G: Gen,
    G::Value: Clone + Debug + Send,
    R: Testable,
    P: Fn(G::Value) -> R + Sync,
{
    let (config, problems) = config.clone().apply_env_vars(std::env::vars());
    let threads = match config.threads {
        0 => std::thread::available_parallelism().map_or(1, |n| n.get()),
        n => n,
    };
    if threads <= 1 {
        return check(&Config { threads: 1, ..config }, generator, property);
    }

    let seed = config.seed.unwrap_or_else(random_seed);
    let name = property_name(&config);
    let mut run = Run::with_seed(config.clone(), ByRef(&generator), &property, seed, name);
    run.report.run = crate::report::RunSpec::current();
    run.report.threads = threads;
    for problem in problems {
        run.report.warn("config_env", problem);
    }
    #[cfg(feature = "coverage")]
    if config.coverage.enabled && crate::coverage::available() {
        run.report
            .warn("coverage_par", "coverage is not collected by check_par; use check for coverage");
    }

    let per_thread = config.tests.div_ceil(threads as u64);
    let worker_configs: Vec<Config> =
        (0..threads).map(|_| Config { tests: per_thread, ..config.clone() }).collect();
    let stop = AtomicBool::new(false);
    let generator_ref = &generator;
    let property_ref = &property;
    let started = run.started;

    let workers: Vec<Run<ByRef<'_, G>, &P>> = std::thread::scope(|scope| {
        let handles: Vec<_> = worker_configs
            .iter()
            .enumerate()
            .map(|(index, worker_config)| {
                let stop = &stop;
                scope.spawn(move || {
                    let worker_seed = mix(seed, u64::MAX - 2 - index as u64);
                    let mut worker = Run::with_seed(
                        worker_config.clone(),
                        ByRef(generator_ref),
                        property_ref,
                        worker_seed,
                        String::new(),
                    );
                    worker.started = started;
                    worker.random_search(Some(stop));
                    if worker.pending.is_some() {
                        stop.store(true, Ordering::Relaxed);
                    }
                    worker
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("worker thread panicked outside the property"))
            .collect()
    });

    let mut failure: Option<(usize, Found<G::Value>)> = None;
    for (index, mut worker) in workers.into_iter().enumerate() {
        let found = worker.pending.take();
        run.absorb(worker);
        if let Some((found, _)) = found
            && failure.is_none()
        {
            failure = Some((index, found));
        }
    }
    if let Some((thread, found)) = failure {
        run.report.status = Status::Passed;
        run.report.abort = None;
        run.pending = Some((found, Some(thread)));
    } else if run.report.status == Status::Passed {
        run.targeted_search();
    }
    run.finish()
}

/// A generator borrowed by the workers of [`check_par`].
struct ByRef<'a, G>(&'a G);

impl<G: Gen> Gen for ByRef<'_, G> {
    type Value = G::Value;

    fn generate(&self, g: &mut Source) -> Self::Value {
        self.0.generate(g)
    }

    fn shrink(&self, value: &Self::Value) -> Vec<Self::Value> {
        self.0.shrink(value)
    }

    fn mutate(&self, value: &Self::Value, g: &mut Source) -> Option<Self::Value> {
        self.0.mutate(value, g)
    }
}

/// Per-run coverage state: the tracker, the fault-localization spectrum and
/// the signature of the failure being localized.
#[cfg(feature = "coverage")]
struct CoverageState {
    tracker: Option<crate::coverage::tracker::Tracker>,
    spectrum: Option<crate::coverage::locate::Spectrum>,
    targets: Option<crate::coverage::targets::Targets>,
    #[cfg(feature = "json")]
    grid: Option<crate::coverage::grid::Grid>,
    signature: Option<Signature>,
    /// `type_name` of the property, so the grid can point at its function.
    property_type: &'static str,
}

#[cfg(feature = "coverage")]
impl CoverageState {
    fn new() -> Self {
        CoverageState {
            tracker: None,
            spectrum: None,
            targets: None,
            #[cfg(feature = "json")]
            grid: None,
            signature: None,
            property_type: "",
        }
    }

    fn begin(&self) -> Option<crate::coverage::Snapshot> {
        self.tracker.as_ref().and_then(|t| t.begin())
    }

    /// Record what an input did: whether it reached new coverage and, when
    /// asked, the source lines it executed. Passing runs and runs failing
    /// like the first failure feed the spectrum; discards and other failures
    /// do not.
    fn end(
        &mut self,
        before: Option<crate::coverage::Snapshot>,
        result: &Result<Outcome, PanicInfo>,
        want_lines: bool,
    ) -> Covered {
        let (Some(tracker), Some(before)) = (self.tracker.as_mut(), before) else {
            return Covered::default();
        };
        let observation = tracker.end(&before);
        let target_hit = match self.targets.as_mut() {
            Some(targets) => targets.observe(&observation.delta, tracker.inputs()),
            None => false,
        };
        #[cfg(feature = "json")]
        if let Some(grid) = self.grid.as_mut() {
            grid.observe(&observation.delta);
        }
        let lines = if want_lines {
            crate::coverage::lines_hit(&observation.delta, tracker.selected())
        } else {
            None
        };
        if let Some(spectrum) = self.spectrum.as_mut() {
            match Signature::of(result) {
                None => {
                    if matches!(result, Ok(Outcome::Pass)) {
                        spectrum.observe(&observation.delta, false);
                    }
                },
                Some(signature) => {
                    let same = match &self.signature {
                        Some(first) => *first == signature,
                        None => {
                            self.signature = Some(signature);
                            true
                        },
                    };
                    if same {
                        spectrum.observe(&observation.delta, true);
                    }
                },
            }
        }
        Covered {
            novel: observation.novel || target_hit,
            novelty: observation.novelty + u32::from(target_hit),
            lines,
        }
    }
}

/// What [`CoverageState::end`] learned about one input.
#[derive(Default)]
struct Covered {
    novel: bool,
    /// Counters, hit-count classes and targets reached for the first time.
    novelty: u32,
    lines: Option<BTreeMap<String, Vec<u32>>>,
}

#[cfg(not(feature = "coverage"))]
struct CoverageState;

#[cfg(not(feature = "coverage"))]
impl CoverageState {
    fn new() -> Self {
        CoverageState
    }

    fn begin(&self) -> Option<()> {
        None
    }

    fn end(
        &mut self,
        _before: Option<()>,
        _result: &Result<Outcome, PanicInfo>,
        _want_lines: bool,
    ) -> Covered {
        Covered::default()
    }
}

/// The crate a type was defined in, read off its `type_name`: the first
/// path segment after any reference or pointer sigils.
#[cfg(feature = "coverage")]
pub(crate) fn crate_of(type_name: &str) -> &str {
    let bare = type_name
        .trim_start_matches(['&', '*', '<'])
        .trim_start_matches("mut ")
        .trim_start_matches("dyn ");
    let end = bare.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(bare.len());
    &bare[..end]
}

#[cfg(all(test, feature = "coverage"))]
fn crate_of_probe_name() -> &'static str {
    let probe = |x: i32| x;
    let _ = probe(0);
    std::any::type_name_of_val(&probe)
}

fn property_name(config: &Config) -> String {
    config
        .name
        .clone()
        .or_else(|| std::thread::current().name().map(str::to_string))
        .unwrap_or_else(|| "unnamed".to_string())
}

/// One property-based test as a sequence of steps, for testing loops
/// written by hand. [`check`] is this loop:
///
/// ```
/// use crabcheck::{Config, Run, gens};
///
/// let config = Config::default().seed(1).tests(200);
/// let mut run = Run::new(&config, gens::vec(gens::int(-100i32..=100)), |v: Vec<i32>| {
///     v.iter().map(|&x| i64::from(x)).sum::<i64>().abs() < 1_000_000
/// });
/// while run.wants_more() {
///     let Some(input) = run.draw() else { continue };
///     let tested = run.test(&input);
///     if tested.passed() {
///         run.keep(input.value, &tested);
///     }
/// }
/// let report = run.finish();
/// report.assert_passed();
/// ```
///
/// [`new`](Self::new) applies the environment overrides, chooses the seed
/// and starts coverage collection when configured. [`draw`](Self::draw)
/// makes a random input, [`mutate`](Self::mutate) a variant of one, and
/// [`test`](Self::test) runs the property on any input, recording the
/// verdict, labels, features, timing and coverage in the report. Passing
/// inputs may be [kept](Self::keep) as seeds for mutation and taken back
/// with [`pool_pick`](Self::pool_pick). [`wants_more`](Self::wants_more)
/// applies the stopping rules (the number of tests, the time budget, the
/// timeout, the discard limit, and a failure). Shrinking is a step too:
/// [`shrink`](Self::shrink) runs the built-in driver on the failure
/// whenever the loop likes, or [`reproduces`](Self::reproduces) and
/// [`shrunk_to`](Self::shrunk_to) let the loop drive its own.
/// [`finish`](Self::finish) shrinks the first failure if nothing did,
/// localizes it, remembers it and writes the report, its files and its
/// pages.
///
/// A loop is reproducible by seed: all randomness comes from the run, so
/// the same seed replays the same run. Inputs from `draw` also carry a
/// [replay point](Input::replay) that regenerates them alone; mutants do
/// not, so a failure found by mutation is remembered by its rendering.
pub struct Run<G: Gen, P> {
    config: Config,
    generator: G,
    property: P,
    report: Report,
    seed: u64,
    /// Regressions key; only read with the `json` feature.
    #[cfg_attr(not(feature = "json"), allow(dead_code))]
    name: String,
    started: Instant,
    seen: HashSet<u64>,
    generated: u64,
    pool: Pool<G::Value>,
    coverage: CoverageState,
    /// `cover` labels: (times met, times seen) over passing tests.
    covers: BTreeMap<String, (u64, u64)>,
    stats: Stats,
    /// Whether any passing input reported a `target` score.
    saw_score: bool,
    /// Observation lines for Tyche, when `Config::report.observations` is set.
    observations: Option<Observations>,
    /// Generation time of the latest input, for its observation line.
    last_gen: std::time::Duration,
    /// The targeted phase's choices: explore or mutate, and which seed.
    rng: ChaCha8Rng,
    /// The first failure, shrunk and reported by [`finish`](Self::finish),
    /// with the worker thread that found it.
    pending: Option<(Found<G::Value>, Option<usize>)>,
    /// Mutations made through [`mutate`](Self::mutate), for the target
    /// summary of hand-written loops.
    mutations: u64,
    /// The pending failure's shrinking, once it was reproduced.
    shrinking: Option<Shrinking<G::Value>>,
    /// Held while coverage is collected, so runs do not interleave.
    #[cfg(feature = "coverage")]
    lease: Option<crate::coverage::Lease>,
}

/// One evaluated input, as recorded in the observation file.
#[cfg_attr(not(feature = "json"), allow(dead_code))]
struct ObservationLine<'a> {
    origin: &'static str,
    seed: u64,
    iteration: u64,
    size: usize,
    representation: String,
    result: &'a Result<Outcome, PanicInfo>,
    observed: Option<&'a labels::Observed>,
    generate: std::time::Duration,
    execute: std::time::Duration,
    coverage: Option<BTreeMap<String, Vec<u32>>>,
}

/// The OpenPBTStats lines of one run (see `Config::report.observations`).
#[cfg(feature = "json")]
struct Observations {
    run_start: f64,
    property: String,
    lines: Vec<String>,
}

#[cfg(feature = "json")]
impl Observations {
    fn new(property: String) -> Observations {
        let run_start = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64());
        Observations { run_start, property, lines: Vec::new() }
    }

    fn record(&mut self, line: ObservationLine<'_>) {
        use serde_json::json;
        let (status, reason) = match line.result {
            Ok(Outcome::Pass) => ("passed", String::new()),
            Ok(Outcome::Discard(reason)) => ("gave_up", reason.clone()),
            Ok(Outcome::False) => ("failed", "the property returned false".to_string()),
            Ok(Outcome::Fail(message)) => ("failed", message.clone()),
            Err(info) => {
                (
                    "failed",
                    match &info.location {
                        Some(at) => format!("panicked at {at}: {}", info.message),
                        None => format!("panicked: {}", info.message),
                    },
                )
            },
        };
        let mut features = serde_json::Map::new();
        if let Some(observed) = line.observed {
            for label in &observed.labels {
                features.insert(label.clone(), json!(""));
            }
            for (name, value) in &observed.features {
                let value = match value {
                    labels::FeatureValue::Number(n) => json!(n),
                    labels::FeatureValue::Text(t) => json!(t),
                };
                features.insert(name.clone(), value);
            }
            for (label, met) in &observed.covers {
                features.insert(format!("cover:{label}"), json!(met.to_string()));
            }
            if let Some(score) = observed.score {
                features.insert("target".to_string(), json!(score));
            }
        }
        let record = json!({
            "type": "test_case",
            "run_start": self.run_start,
            "property": self.property,
            "status": status,
            "status_reason": reason,
            "representation": line.representation,
            "how_generated": line.origin,
            "features": features,
            "coverage": line.coverage,
            "timing": {
                "generate:input": line.generate.as_secs_f64(),
                "execute:test": line.execute.as_secs_f64(),
            },
            "metadata": {
                "seed": line.seed,
                "iteration": line.iteration,
                "size": line.size,
                "origin": line.origin,
            },
        });
        self.lines.push(record.to_string());
    }

    fn info(&mut self, title: &str, content: &str) {
        let record = serde_json::json!({
            "type": "info",
            "run_start": self.run_start,
            "property": self.property,
            "title": title,
            "content": content,
        });
        self.lines.push(record.to_string());
    }

    /// Append the lines to `<dir>/<property>.jsonl`.
    fn write(&self, dir: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
        use std::io::Write as _;
        std::fs::create_dir_all(dir)?;
        let path = dir.join(crate::regressions::file_name(&self.property));
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
        for line in &self.lines {
            writeln!(file, "{line}")?;
        }
        Ok(path)
    }
}

#[cfg(not(feature = "json"))]
struct Observations;

#[cfg(not(feature = "json"))]
impl Observations {
    fn record(&mut self, _line: ObservationLine<'_>) {}
}

/// Where a sampled or slow input can be rendered from later.
#[derive(Clone)]
enum Provenance {
    /// Regenerate from the point (random-phase inputs).
    Point(ReplayPoint),
    /// Already rendered (mutants).
    Debug(String),
}

#[derive(Clone)]
struct Candidate {
    origin: crate::report::Origin,
    size: usize,
    iteration: u64,
    /// The uniform draw that decided its reservoir slot; kept so workers'
    /// candidates can be re-considered when runs merge.
    r: f64,
    source: Provenance,
}

/// The inputs shown as samples: the smallest and largest by size plus a
/// reservoir sample of the rest.
struct Samples {
    limit: usize,
    smallest: Option<Candidate>,
    largest: Option<Candidate>,
    reservoir: Vec<Candidate>,
    seen: u64,
}

impl Samples {
    fn new(limit: usize) -> Samples {
        Samples { limit, smallest: None, largest: None, reservoir: Vec::new(), seen: 0 }
    }

    /// Offer a passing input; `source` is only called when it is kept.
    fn consider(
        &mut self,
        size: usize,
        iteration: u64,
        origin: crate::report::Origin,
        r: f64,
        source: impl FnOnce() -> Provenance,
    ) {
        if self.limit == 0 {
            return;
        }
        self.seen += 1;
        let smaller = self.smallest.as_ref().is_none_or(|c| size < c.size);
        let larger = self.limit >= 2 && self.largest.as_ref().is_none_or(|c| size > c.size);
        let k = self.limit.saturating_sub(2);
        let slot = if k == 0 {
            None
        } else if self.reservoir.len() < k {
            Some(self.reservoir.len())
        } else {
            let j = (r * self.seen as f64) as usize;
            (j < k).then_some(j)
        };
        if !(smaller || larger || slot.is_some()) {
            return;
        }
        let candidate = Candidate { origin, size, iteration, r, source: source() };
        if smaller {
            self.smallest = Some(candidate.clone());
        }
        if larger {
            self.largest = Some(candidate.clone());
        }
        match slot {
            Some(j) if j < self.reservoir.len() => self.reservoir[j] = candidate,
            Some(_) => self.reservoir.push(candidate),
            None => {},
        }
    }

    /// Everything kept, smallest first, without repeats.
    fn drain(&mut self) -> Vec<Candidate> {
        let mut out: Vec<Candidate> = Vec::new();
        for candidate in self
            .smallest
            .take()
            .into_iter()
            .chain(self.largest.take())
            .chain(self.reservoir.drain(..))
        {
            if !out
                .iter()
                .any(|c| c.origin == candidate.origin && c.iteration == candidate.iteration)
            {
                out.push(candidate);
            }
        }
        out
    }
}

/// Per-run accumulators behind the report's distributions.
struct Stats {
    /// Debug hashes of passing inputs.
    passed_seen: HashSet<u64>,
    /// Numeric feature values over passing tests.
    features: BTreeMap<String, Vec<f64>>,
    gen_micros: Vec<u32>,
    prop_micros: Vec<u32>,
    slowest: Option<(u32, Provenance)>,
    distinct_growth: crate::report::Series,
    samples: Samples,
}

impl Stats {
    fn new(samples: usize) -> Stats {
        Stats {
            passed_seen: HashSet::new(),
            features: BTreeMap::new(),
            gen_micros: Vec::new(),
            prop_micros: Vec::new(),
            slowest: None,
            distinct_growth: crate::report::Series::default(),
            samples: Samples::new(samples),
        }
    }

    /// Note an input's property time; `source` is only called for a new
    /// maximum.
    fn slower(&mut self, elapsed: std::time::Duration, source: impl FnOnce() -> Provenance) {
        let micros = micros(elapsed);
        if self.slowest.as_ref().is_none_or(|(best, _)| micros > *best) {
            self.slowest = Some((micros, source()));
        }
    }
}

/// Samples are for reading, so they are cut shorter than counterexamples.
const SAMPLE_CHARS: usize = 200;

/// Cut a rendering to `limit` characters, marking the cut.
fn cap(mut debug: String, limit: usize) -> String {
    if let Some((cut, _)) = debug.char_indices().nth(limit) {
        debug.truncate(cut);
        debug.push('…');
    }
    debug
}

fn micros(d: std::time::Duration) -> u32 {
    d.as_micros().min(u128::from(u32::MAX)) as u32
}

/// A uniform draw in `[0, 1)` from a seed and an index.
fn hash_u01(seed: u64, index: u64) -> f64 {
    (mix(seed, index) >> 11) as f64 / (1u64 << 53) as f64
}

/// A failing input found by the random or targeted phase.
struct Found<T> {
    value: T,
    /// Where the input regenerates from; `None` for mutants.
    point: Option<ReplayPoint>,
    size: usize,
    iteration: u64,
    result: Result<Outcome, PanicInfo>,
}

/// The pending failure's shrinking: what it fails like, the smallest input
/// known to fail the same way, and the steps taken so far.
struct Shrinking<T> {
    signature: Signature,
    /// The failure did not reproduce when rerun; nothing is shrunk.
    flaky: bool,
    minimal: T,
    /// The message and location of the failure `minimal` produced, once a
    /// candidate was accepted.
    failure: shrink::SameFailure,
    stats: ShrinkStats,
    /// The built-in driver ran, or the loop supplied its own result.
    done: bool,
    started: Option<Instant>,
}

/// Run the property on a shrink candidate and say whether it failed like
/// the original (with the failure's message and location); the candidate
/// is recorded in the observation file. Returns the time it took.
#[allow(clippy::too_many_arguments)]
fn try_candidate<T: Debug, R: Testable, P: FnMut(T) -> R>(
    property: &mut P,
    coverage: &mut CoverageState,
    observations: &mut Option<Observations>,
    seed: u64,
    iteration: u64,
    size: usize,
    signature: &Signature,
    candidate: T,
) -> (shrink::SameFailure, std::time::Duration) {
    let observing = observations.is_some();
    let representation = if observing { render(&candidate, usize::MAX).0 } else { String::new() };
    let before = coverage.begin();
    let ran = Instant::now();
    let (outcome, observed) = evaluate(property, candidate);
    let elapsed = ran.elapsed();
    let covered = coverage.end(before, &outcome, observing);
    if let Some(observations) = observations.as_mut() {
        observations.record(ObservationLine {
            origin: "shrink",
            seed,
            iteration,
            size,
            representation,
            result: &outcome,
            observed: Some(&observed),
            generate: std::time::Duration::ZERO,
            execute: elapsed,
            coverage: covered.lines,
        });
    }
    let same = (Signature::of(&outcome).as_ref() == Some(signature)).then(|| {
        let (_, message, location) = details(outcome);
        (message, location)
    });
    (same, elapsed)
}

/// Why an input could not be generated.
enum Skipped {
    Discarded(String),
    Panicked(PanicInfo),
}

/// An input drawn or mutated by a [`Run`], ready to be tested.
#[derive(Clone, Debug)]
pub struct Input<T> {
    /// The value.
    pub value: T,
    point: Option<ReplayPoint>,
    size: usize,
    iteration: u64,
    origin: crate::report::Origin,
    generate: std::time::Duration,
}

impl<T> Input<T> {
    /// The size the generator was asked for.
    pub fn size(&self) -> usize {
        self.size
    }

    /// The run's iteration that produced it.
    pub fn iteration(&self) -> u64 {
        self.iteration
    }

    /// Where it regenerates from, for inputs from [`Run::draw`]; mutants
    /// have none.
    pub fn replay(&self) -> Option<ReplayPoint> {
        self.point
    }

    /// How it came to be.
    pub fn origin(&self) -> crate::report::Origin {
        self.origin
    }
}

/// The three ways testing one input can end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The property held.
    Passed,
    /// The property discarded the input.
    Discarded,
    /// The property failed; the run keeps the first such input for
    /// [`Run::finish`].
    Failed,
}

/// What one test of an input showed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tested {
    verdict: Verdict,
    novel: bool,
    novelty: u32,
    score: Option<f64>,
    elapsed: std::time::Duration,
}

impl Tested {
    /// How the test ended.
    pub fn verdict(&self) -> Verdict {
        self.verdict
    }

    /// Whether the property held.
    pub fn passed(&self) -> bool {
        self.verdict == Verdict::Passed
    }

    /// Whether the property discarded the input.
    pub fn discarded(&self) -> bool {
        self.verdict == Verdict::Discarded
    }

    /// Whether the property failed.
    pub fn failed(&self) -> bool {
        self.verdict == Verdict::Failed
    }

    /// Whether the input executed code no earlier input had (coverage on,
    /// instrumented build); otherwise always false.
    pub fn novel(&self) -> bool {
        self.novel
    }

    /// How many counters, hit-count classes of a counter, or coverage
    /// targets the input reached for the first time in this run: the size
    /// of its novelty. Zero without coverage.
    pub fn novelty(&self) -> u32 {
        self.novelty
    }

    /// How long the property took on the input.
    pub fn elapsed(&self) -> std::time::Duration {
        self.elapsed
    }

    /// The score the property reported through [`target`](crate::target),
    /// for passing inputs.
    pub fn score(&self) -> Option<f64> {
        self.score
    }
}

impl<G, R, P> Run<G, P>
where
    G: Gen,
    G::Value: Clone + Debug,
    R: Testable,
    P: FnMut(G::Value) -> R,
{
    /// Set up a run: the environment overrides applied on top of `config`,
    /// the seed chosen (or taken from the config), coverage collection
    /// started when configured and the binary is instrumented, and the
    /// report begun.
    pub fn new(config: &Config, generator: G, property: P) -> Self {
        let (config, problems) = config.clone().apply_env_vars(std::env::vars());
        let seed = config.seed.unwrap_or_else(random_seed);
        let name = property_name(&config);
        let mut run = Run::with_seed(config, generator, property, seed, name);
        run.report.run = crate::report::RunSpec::current();
        for problem in problems {
            run.report.warn("config_env", problem);
        }
        #[cfg(feature = "coverage")]
        {
            run.lease = run.enable_coverage();
        }
        #[cfg(not(feature = "coverage"))]
        run.enable_coverage();
        #[cfg(not(feature = "json"))]
        if run.config.report.observations.is_some() {
            run.report.warn("observations", "observation files need the `json` feature");
        }
        run
    }

    fn with_seed(config: Config, generator: G, property: P, seed: u64, name: String) -> Self {
        let report = Report { name: name.clone(), ..Report::new(seed, config.max_size) };
        let pool = Pool::new(config.target.pool_size, config.target.energy);
        let stats = Stats::new(config.report.samples);
        #[cfg(feature = "json")]
        let observations =
            config.report.observations.as_ref().map(|_| Observations::new(name.clone()));
        #[cfg(not(feature = "json"))]
        let observations = None;
        Run {
            config,
            generator,
            property,
            report,
            seed,
            name,
            started: Instant::now(),
            seen: HashSet::new(),
            generated: 0,
            pool,
            coverage: CoverageState::new(),
            covers: BTreeMap::new(),
            stats,
            observations,
            last_gen: std::time::Duration::ZERO,
            saw_score: false,
            rng: ChaCha8Rng::seed_from_u64(mix(seed, u64::MAX - 1)),
            pending: None,
            mutations: 0,
            shrinking: None,
            #[cfg(feature = "coverage")]
            lease: None,
        }
    }

    /// Generate the input at `point`. Accumulates generation time.
    fn generate(&mut self, point: ReplayPoint) -> Result<(G::Value, usize), Skipped> {
        let size = size_for(point.seed, point.iteration, self.config.max_size);
        let mut source = Source::new(mix(point.seed, point.iteration), size)
            .with_filter_max_attempts(self.config.filter_max_attempts);
        let started = Instant::now();
        let generated = panic::catch(|| self.generator.generate(&mut source));
        let elapsed = started.elapsed();
        self.report.gen_time += elapsed;
        self.stats.gen_micros.push(micros(elapsed));
        self.last_gen = elapsed;
        match generated {
            Err(info) => Err(Skipped::Panicked(info)),
            Ok(_) if source.discarded().is_some() => {
                Err(Skipped::Discarded(source.discarded().unwrap_or_default().to_string()))
            },
            Ok(value) => Ok((value, size)),
        }
    }

    /// The value at `point`, if it generates without discard or panic.
    fn regenerate(&mut self, point: ReplayPoint) -> Option<G::Value> {
        self.generate(point).ok().map(|(value, _)| value)
    }

    /// Run the property once. Accumulates property time.
    fn evaluate(
        &mut self,
        value: G::Value,
    ) -> (Result<Outcome, PanicInfo>, labels::Observed, std::time::Duration) {
        let started = Instant::now();
        let (result, observed) = evaluate(&mut self.property, value);
        let elapsed = started.elapsed();
        self.report.prop_time += elapsed;
        self.stats.prop_micros.push(micros(elapsed));
        (result, observed, elapsed)
    }

    /// Add a line to the observation file, when one is being written.
    fn observe(&mut self, line: ObservationLine<'_>) {
        if let Some(observations) = self.observations.as_mut() {
            observations.record(line);
        }
    }

    /// Count a generated input, and the growth of the distinct set.
    fn note_distinct(&mut self, hash: u64) {
        if self.seen.insert(hash) {
            self.stats.distinct_growth.push(self.generated, self.seen.len() as u64);
        }
    }

    fn next_iteration(&mut self) -> u64 {
        let iteration = self.report.iterations;
        self.report.iterations += 1;
        iteration
    }

    /// The observation line of an input the generator discarded.
    fn observe_generation_discard(
        &mut self,
        point: ReplayPoint,
        origin: crate::report::Origin,
        reason: &str,
    ) {
        if self.observations.is_none() {
            return;
        }
        let result = Ok(Outcome::Discard(reason.to_string()));
        self.observe(ObservationLine {
            origin: origin.as_str(),
            seed: point.seed,
            iteration: point.iteration,
            size: size_for(point.seed, point.iteration, self.config.max_size),
            representation: String::new(),
            result: &result,
            observed: None,
            generate: self.last_gen,
            execute: std::time::Duration::ZERO,
            coverage: None,
        });
    }

    /// Draw the input at `point`. A discard by the generator is counted
    /// and a generator panic aborts the run; both yield `None`.
    fn draw_at(
        &mut self,
        point: ReplayPoint,
        origin: crate::report::Origin,
    ) -> Option<Input<G::Value>> {
        match self.generate(point) {
            Ok((value, size)) => {
                Some(Input {
                    value,
                    point: Some(point),
                    size,
                    iteration: point.iteration,
                    origin,
                    generate: self.last_gen,
                })
            },
            Err(Skipped::Discarded(reason)) => {
                self.observe_generation_discard(point, origin, &reason);
                self.record_discard(reason);
                None
            },
            Err(Skipped::Panicked(info)) => {
                self.report.status = Status::Aborted;
                self.report.abort = Some(Abort {
                    message: info.message,
                    location: info.location,
                    iteration: point.iteration,
                });
                None
            },
        }
    }

    /// Draw the next random input, at a size sampled for this iteration.
    /// `None` when the generator discarded it (counted toward the discard
    /// limit) or panicked (the run aborts; see [`wants_more`](Self::wants_more)).
    pub fn draw(&mut self) -> Option<Input<G::Value>> {
        let iteration = self.next_iteration();
        self.draw_at(ReplayPoint { seed: self.seed, iteration }, crate::report::Origin::Random)
    }

    /// Run the property on `input`. A pass counts toward the run's tests
    /// and records the input's labels, features, timing and coverage; a
    /// discard counts toward the discard limit; the first failure is kept
    /// for [`finish`](Self::finish) to shrink and report, and later ones
    /// are only counted.
    pub fn test(&mut self, input: &Input<G::Value>) -> Tested {
        let (tested, result) = self.test_input(input, true);
        if tested.failed() && self.pending.is_none() {
            self.pending = Some((
                Found {
                    value: input.value.clone(),
                    point: input.point,
                    size: input.size,
                    iteration: input.iteration,
                    result,
                },
                None,
            ));
        }
        tested
    }

    /// Test `input`, counting passes and discards when `record`. Returns
    /// the raw result alongside the verdict for callers that keep failures
    /// themselves.
    fn test_input(
        &mut self,
        input: &Input<G::Value>,
        record: bool,
    ) -> (Tested, Result<Outcome, PanicInfo>) {
        let observing = self.observations.is_some();
        self.generated += 1;
        let hash = debug_hash(&input.value);
        self.note_distinct(hash);
        let representation =
            if observing { render(&input.value, usize::MAX).0 } else { String::new() };
        let before = self.coverage.begin();
        let (result, observed, elapsed) = self.evaluate(input.value.clone());
        let covered = self.coverage.end(before, &result, observing);
        let novel = covered.novel;
        let novelty = covered.novelty;
        let limit = self.config.report.truncate_debug_at;
        let (value, point) = (&input.value, input.point);
        let provenance = || {
            match point {
                Some(point) => Provenance::Point(point),
                None => Provenance::Debug(render(value, limit).0),
            }
        };
        self.stats.slower(elapsed, provenance);
        if observing {
            self.observe(ObservationLine {
                origin: input.origin.as_str(),
                seed: point.map_or(self.seed, |p| p.seed),
                iteration: input.iteration,
                size: input.size,
                representation,
                result: &result,
                observed: Some(&observed),
                generate: input.generate,
                execute: elapsed,
                coverage: covered.lines,
            });
        }
        let score = observed.score;
        match result {
            Ok(Outcome::Pass) => {
                if record {
                    self.record_pass(
                        input.size,
                        hash,
                        observed,
                        input.origin,
                        input.iteration,
                        provenance,
                    );
                }
                (
                    Tested { verdict: Verdict::Passed, novel, novelty, score, elapsed },
                    Ok(Outcome::Pass),
                )
            },
            Ok(Outcome::Discard(reason)) => {
                if record {
                    self.record_discard(reason.clone());
                }
                (
                    Tested { verdict: Verdict::Discarded, novel, novelty, score: None, elapsed },
                    Ok(Outcome::Discard(reason)),
                )
            },
            result => {
                (Tested { verdict: Verdict::Failed, novel, novelty, score: None, elapsed }, result)
            },
        }
    }

    fn record_discard(&mut self, reason: String) {
        self.report.discarded += 1;
        *self.report.discard_reasons.entry(reason).or_default() += 1;
    }

    fn record_pass(
        &mut self,
        size: usize,
        hash: u64,
        observed: labels::Observed,
        origin: crate::report::Origin,
        iteration: u64,
        source: impl FnOnce() -> Provenance,
    ) {
        self.report.tests += 1;
        self.stats.passed_seen.insert(hash);
        *self.report.sizes.entry(size).or_default() += 1;
        for label in observed.labels {
            *self.report.labels.entry(label).or_default() += 1;
        }
        for (label, met) in observed.covers {
            let entry = self.covers.entry(label).or_insert((0, 0));
            entry.0 += u64::from(met);
            entry.1 += 1;
        }
        for (name, value) in observed.features {
            match value {
                labels::FeatureValue::Number(n) => {
                    self.stats.features.entry(name).or_default().push(n);
                },
                labels::FeatureValue::Text(text) => {
                    *self.report.labels.entry(format!("{name}={text}")).or_default() += 1;
                },
            }
        }
        if let Some(score) = observed.score {
            self.saw_score = true;
            self.stats.features.entry("target".to_string()).or_default().push(score);
        }
        let r = hash_u01(self.seed ^ 0x5a5a_5a5a, self.report.tests);
        self.stats.samples.consider(size, iteration, origin, r, source);
    }

    /// Whether coverage-guided search is on and coverage is being collected.
    fn guided(&self) -> bool {
        #[cfg(feature = "coverage")]
        {
            (self.config.coverage.guided || self.coverage.targets.is_some())
                && self.coverage.tracker.is_some()
        }
        #[cfg(not(feature = "coverage"))]
        {
            false
        }
    }

    #[cfg(feature = "coverage")]
    /// Start collecting coverage if configured and available. Returns the
    /// process-wide coverage lock, which the caller holds for the run.
    fn enable_coverage(&mut self) -> Option<crate::coverage::Lease> {
        let settings = &self.config.coverage;
        if !settings.enabled {
            return None;
        }
        if !crate::coverage::available() {
            if settings.guided
                || settings.profraw_dir.is_some()
                || !settings.targets.is_empty()
                || settings.html.is_some()
            {
                self.report.warn(
                    "coverage_unavailable",
                    "coverage was requested but this binary is not instrumented; build with RUSTFLAGS=\"-C instrument-coverage\"",
                );
            }
            return None;
        }
        self.coverage.property_type = std::any::type_name::<P>();
        let filter = if settings.filter.is_empty() {
            crate_of(std::any::type_name::<P>()).to_string()
        } else {
            settings.filter.clone()
        };
        let targets = self.resolve_targets();
        let mut extra = targets.as_ref().map(|t| t.functions()).unwrap_or_default();
        // The property's own function is tracked whichever crate it lives
        // in, so the grid can point at it; it stays out of fault
        // localization, where it could only be noise.
        let property_fns: Vec<usize> = crate::coverage::counters::Functions::get()
            .all()
            .iter()
            .enumerate()
            .filter(|(_, f)| crate::coverage::is_property_fn(&f.name, self.coverage.property_type))
            .map(|(i, _)| i)
            .collect();
        extra.extend(property_fns.iter().copied());
        match crate::coverage::tracker::Tracker::new(&filter, &extra) {
            Some(tracker) => {
                let guard = crate::coverage::lock();
                self.coverage.targets = targets;
                if self.config.coverage.html.is_some() {
                    #[cfg(feature = "json")]
                    {
                        self.coverage.grid = crate::coverage::grid::Grid::new(tracker.selected());
                        if self.coverage.grid.is_none() {
                            self.report.warn(
                                "coverage_html",
                                "the binary carries no coverage mapping, so the coverage grid cannot be written",
                            );
                        }
                    }
                    #[cfg(not(feature = "json"))]
                    self.report.warn("coverage_html", "the coverage grid needs the `json` feature");
                }
                if self.config.locate.enabled {
                    let code_under_test: Vec<usize> = tracker
                        .selected()
                        .iter()
                        .copied()
                        .filter(|i| !property_fns.contains(i))
                        .collect();
                    self.coverage.spectrum =
                        crate::coverage::locate::Spectrum::new(&code_under_test);
                    if self.coverage.spectrum.is_none() {
                        self.report.warn(
                            "locate_unavailable",
                            "the binary carries no coverage mapping, so failures cannot be localized",
                        );
                    }
                }
                self.coverage.tracker = Some(tracker);
                // The test harness runs each test on a thread named after it;
                // a plain binary runs on `main` and has no sibling tests.
                let in_test_harness = std::thread::current().name().is_some_and(|n| n != "main");
                if in_test_harness
                    && std::env::var("RUST_TEST_THREADS").ok().as_deref() != Some("1")
                {
                    self.report.warn(
                        "coverage_threads",
                        "coverage is attributed per input, and other test threads running at the same time pollute it; \
                         run with RUST_TEST_THREADS=1 for exact attribution",
                    );
                }
                Some(guard)
            },
            None => {
                self.report.warn(
                    "coverage_filter",
                    format!("no instrumented functions match the coverage filter `{filter}`"),
                );
                None
            },
        }
    }

    /// The configured coverage targets (inline and from the file), resolved.
    #[cfg(feature = "coverage")]
    fn resolve_targets(&mut self) -> Option<crate::coverage::targets::Targets> {
        #[cfg_attr(not(feature = "json"), allow(unused_mut))]
        let mut targets = self.config.coverage.targets.clone();
        if let Some(path) = self.config.coverage.targets_file.clone() {
            #[cfg(feature = "json")]
            match crate::config::load_targets(&path) {
                Ok(more) => targets.extend(more),
                Err(e) => {
                    self.report
                        .warn("coverage_targets", format!("could not read the targets file: {e}"))
                },
            }
            #[cfg(not(feature = "json"))]
            if path.exists() {
                self.report.warn(
                    "coverage_targets",
                    format!("reading {} needs the `json` feature", path.display()),
                );
            }
        }
        if targets.is_empty() {
            return None;
        }
        match crate::coverage::targets::Targets::resolve(targets) {
            Some(resolved) => {
                for target in resolved.unresolved() {
                    self.report.warn(
                        "coverage_target",
                        format!(
                            "target {target} matches no instrumented code; check the path and lines"
                        ),
                    );
                }
                Some(resolved)
            },
            None => {
                self.report.warn(
                    "coverage_targets_unavailable",
                    "the binary carries no coverage mapping, so coverage targets cannot be resolved",
                );
                None
            },
        }
    }

    #[cfg(not(feature = "coverage"))]
    fn enable_coverage(&mut self) -> Option<()> {
        None
    }


    /// Whether the run still wants inputs: none of the stopping rules has
    /// fired. The rules, in order: the number of tests reached
    /// [`Config::tests`]; the time budget ([`Config::budget_ms`]) ran out;
    /// the timeout ([`Config::timeout_ms`]) elapsed; more inputs were
    /// discarded than [`Config::max_discard_ratio`] allows; the property
    /// failed, or the generator panicked. A rule that fired is reflected in
    /// the report.
    pub fn wants_more(&mut self) -> bool {
        if self.pending.is_some() || self.report.status != Status::Passed {
            return false;
        }
        if self.report.tests >= self.config.tests {
            return false;
        }
        if let Some(budget) = self.config.budget_ms.map(std::time::Duration::from_millis)
            && self.started.elapsed() > budget
        {
            self.report.stopped_by_budget = true;
            return false;
        }
        if let Some(timeout) = self.config.timeout_duration()
            && self.started.elapsed() > timeout
        {
            self.report.status = Status::TimedOut;
            return false;
        }
        let max_discards =
            (self.config.max_discard_ratio * self.config.tests as f64).max(0.0) as u64;
        if self.report.discarded > max_discards {
            self.report.status = Status::GaveUp;
            return false;
        }
        true
    }

    /// The random phase: draw, test, keep, until a stopping rule fires.
    /// `stop` lets parallel workers end early once another found a failure.
    fn random_search(&mut self, stop: Option<&AtomicBool>) {
        while self.wants_more() {
            if stop.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                return;
            }
            let iteration = self.next_iteration();
            let point = ReplayPoint { seed: self.seed, iteration };
            let Some(input) = self.draw_at(point, crate::report::Origin::Random) else {
                continue;
            };
            let tested = self.test(&input);
            if tested.passed() {
                self.keep(input.value, &tested);
            }
        }
    }

    /// Keep a passing input as a seed for mutation when its score improves
    /// the pool, or when guided search is on and it reached new coverage.
    /// Returns whether it was kept.
    pub fn keep(&mut self, value: G::Value, tested: &Tested) -> bool {
        if !tested.passed() {
            return false;
        }
        let guided = self.guided();
        let by_score = tested.score.is_some_and(|s| self.pool.improves(s));
        if by_score {
            self.pool.add(value, tested.score.unwrap_or(0.0));
            true
        } else if tested.novel && guided {
            self.pool.add_forced(value, tested.score.unwrap_or(0.0));
            true
        } else {
            false
        }
    }

    /// Keep an input as a seed with `score`, whatever the pool's threshold.
    pub fn keep_always(&mut self, value: G::Value, score: f64) {
        self.pool.add_forced(value, score);
    }

    /// Whether the pool holds seeds to mutate.
    pub fn has_seeds(&self) -> bool {
        !self.pool.is_empty()
    }

    /// Pick a seed to mutate, weighted by score; each seed has a budget of
    /// picks. `None` when the pool is empty or every budget is spent.
    pub fn pool_pick(&mut self) -> Option<G::Value> {
        self.pool.pick(&mut self.rng).map(|seed| seed.value)
    }

    /// A variant of `parent` from the generator's mutator, at a size
    /// sampled for this iteration. `None` when the mutator has nothing to
    /// offer or panicked (the run aborts).
    pub fn mutate(&mut self, parent: &G::Value) -> Option<Input<G::Value>> {
        let iteration = self.next_iteration();
        self.mutations += 1;
        self.mutate_at(parent, iteration)
    }

    fn mutate_at(&mut self, parent: &G::Value, iteration: u64) -> Option<Input<G::Value>> {
        let size = size_for(self.seed, iteration, self.config.max_size);
        let mut source = Source::new(mix(self.seed, iteration), size)
            .with_filter_max_attempts(self.config.filter_max_attempts);
        let started = Instant::now();
        match panic::catch(|| self.generator.mutate(parent, &mut source)) {
            Ok(Some(value)) => {
                Some(Input {
                    value,
                    point: None,
                    size,
                    iteration,
                    origin: crate::report::Origin::Mutation,
                    generate: started.elapsed(),
                })
            },
            Ok(None) => None,
            Err(info) => {
                self.report.status = Status::Aborted;
                self.report.abort =
                    Some(Abort { message: info.message, location: info.location, iteration });
                None
            },
        }
    }

    /// The generator, for loops that shrink or mutate by hand.
    pub fn generator(&self) -> &G {
        &self.generator
    }

    /// The failing input kept by [`test`](Self::test), as found.
    pub fn failure(&self) -> Option<&G::Value> {
        self.pending.as_ref().map(|(found, _)| &found.value)
    }

    /// Rerun the pending failure once to see that it reproduces (the
    /// flakiness check) and set up shrinking. Returns whether there is a
    /// failure to shrink.
    fn reproduce_pending(&mut self) -> bool {
        if let Some(shrinking) = &self.shrinking {
            return !shrinking.flaky;
        }
        let Some((found, _)) = self.pending.as_ref() else {
            return false;
        };
        let signature = Signature::of(&found.result).expect("pending failures fail");
        let value = found.value.clone();
        let rerun = self.evaluate(value.clone()).0;
        let flaky = Signature::of(&rerun).as_ref() != Some(&signature);
        if flaky {
            self.report.warn(
                "flaky",
                "the property did not fail the same way when re-run on the failing input; \
                 it depends on something other than the input, and the input was not shrunk",
            );
        }
        self.shrinking = Some(Shrinking {
            signature,
            flaky,
            minimal: value,
            failure: None,
            stats: ShrinkStats::default(),
            done: false,
            started: None,
        });
        !flaky
    }

    /// Whether `candidate` fails the way the pending failure did (same
    /// kind of failure, same panic site), for loops that shrink by hand.
    /// Each call is a shrinking step: beyond [`Config::shrink`]'s budget
    /// the answer is always `false` and the report says the budget ran
    /// out. `false` without a failure, or when it did not reproduce.
    pub fn reproduces(&mut self, candidate: &G::Value) -> bool {
        if !self.reproduce_pending() {
            return false;
        }
        let (iteration, size) =
            self.pending.as_ref().map_or((0, 0), |(found, _)| (found.iteration, found.size));
        let Some(shrinking) = self.shrinking.as_mut() else {
            return false;
        };
        let started = *shrinking.started.get_or_insert_with(Instant::now);
        let budget = &self.config.shrink;
        if shrinking.stats.steps >= budget.max_steps
            || budget
                .timeout_ms
                .is_some_and(|ms| started.elapsed() >= std::time::Duration::from_millis(ms))
        {
            shrinking.stats.budget_exhausted = true;
            return false;
        }
        shrinking.stats.steps += 1;
        let (same, elapsed) = try_candidate(
            &mut self.property,
            &mut self.coverage,
            &mut self.observations,
            self.seed,
            iteration,
            size,
            &shrinking.signature,
            candidate.clone(),
        );
        self.report.prop_time += elapsed;
        if same.is_some() {
            shrinking.stats.accepted += 1;
        }
        shrinking.stats.duration = started.elapsed();
        same.is_some()
    }

    /// Record `minimal` as the result of the loop's own shrinking. It is
    /// tested once more: when it fails the way the original did, the report
    /// shows it as the shrunk counterexample and [`finish`](Self::finish)
    /// shrinks no further; otherwise the original stands, with a warning.
    /// Returns whether it was accepted.
    pub fn shrunk_to(&mut self, minimal: G::Value) -> bool {
        if !self.reproduce_pending() {
            return false;
        }
        let (iteration, size) =
            self.pending.as_ref().map_or((0, 0), |(found, _)| (found.iteration, found.size));
        let Some(mut shrinking) = self.shrinking.take() else {
            return false;
        };
        let started = *shrinking.started.get_or_insert_with(Instant::now);
        shrinking.stats.steps += 1;
        let (same, elapsed) = try_candidate(
            &mut self.property,
            &mut self.coverage,
            &mut self.observations,
            self.seed,
            iteration,
            size,
            &shrinking.signature,
            minimal.clone(),
        );
        self.report.prop_time += elapsed;
        shrinking.stats.duration = started.elapsed();
        shrinking.done = true;
        let accepted = same.is_some();
        match same {
            Some(failure) => {
                shrinking.stats.accepted += 1;
                shrinking.minimal = minimal;
                shrinking.failure = Some(failure);
            },
            None => {
                self.report.warn(
                    "shrunk_to",
                    "the input given to shrunk_to does not fail the way the original did; \
                     the original is reported",
                );
            },
        }
        self.shrinking = Some(shrinking);
        accepted
    }

    /// Shrink the pending failure now with the built-in driver: greedy over
    /// the generator's candidates, within [`Config::shrink`]'s budget.
    /// Returns the smallest input that fails the same way; `None` without
    /// a failure, or when it did not reproduce. [`finish`](Self::finish)
    /// then reports this result and shrinks no further.
    pub fn shrink(&mut self) -> Option<&G::Value> {
        if !self.reproduce_pending() {
            return None;
        }
        let (iteration, size) =
            self.pending.as_ref().map_or((0, 0), |(found, _)| (found.iteration, found.size));
        let mut shrinking = self.shrinking.take()?;
        if !shrinking.done && self.config.shrink.enabled {
            self.drive_shrink(&mut shrinking, iteration, size);
        }
        self.shrinking = Some(shrinking);
        self.shrinking.as_ref().map(|s| &s.minimal)
    }

    /// The built-in shrinking driver over `shrinking.minimal`.
    fn drive_shrink(&mut self, shrinking: &mut Shrinking<G::Value>, iteration: u64, size: usize) {
        let limit = self.config.report.truncate_debug_at;
        let generator = &self.generator;
        let property = &mut self.property;
        let coverage = &mut self.coverage;
        let observations = &mut self.observations;
        let seed = self.seed;
        let signature = &shrinking.signature;
        let started = Instant::now();
        let result = shrink::shrink(
            &self.config.shrink,
            shrinking.minimal.clone(),
            |value| generator.shrink(value),
            |candidate| {
                try_candidate(
                    property,
                    coverage,
                    observations,
                    seed,
                    iteration,
                    size,
                    signature,
                    candidate,
                )
                .0
            },
            |value| render(value, limit),
        );
        self.report.prop_time += started.elapsed();
        shrinking.minimal = result.value;
        shrinking.failure = result.failure;
        shrinking.stats = result.stats;
        shrinking.done = true;
        if shrinking.stats.budget_exhausted {
            self.report.warn(
                "shrink_budget",
                format!(
                    "shrinking stopped at the budget ({} candidates); raise Config::shrink.max_steps or the \
                     timeout for a smaller counterexample",
                    shrinking.stats.steps
                ),
            );
        }
    }

    /// The fault-localization ranking as it stands: the functions and
    /// regions whose coverage separates the failing inputs tested so far
    /// from the passing ones, with the failure's panic site as a prior.
    /// `None` without a failure, or without coverage (`coverage` feature,
    /// instrumented build, [`Config::locate`] on).
    #[cfg(feature = "coverage")]
    pub fn suspects(&self) -> Option<crate::coverage::Locate> {
        let spectrum = self.coverage.spectrum.as_ref()?;
        let (found, _) = self.pending.as_ref()?;
        let signature = Signature::of(&found.result)?;
        let panic = signature.panic_site.as_ref().map(|(file, line)| (file.as_str(), *line));
        Some(spectrum.rank(panic, self.config.locate.top))
    }

    /// The report so far.
    pub fn report(&self) -> &Report {
        &self.report
    }

    /// The seed of this run.
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Test the remembered inputs first: the replay point
    /// ([`Config::replay`], `CRABCHECK_REPLAY`) and the property's stored
    /// regressions. Neither counts as a test. Returns whether one failed,
    /// in which case the loop should stop and [`finish`](Self::finish)
    /// reports it.
    pub fn remembered(&mut self) -> bool {
        if let Some(point) = self.config.replay {
            let origin = crate::report::Origin::Replay;
            let failed = match self.generate(point) {
                Ok((value, size)) => {
                    let input = Input {
                        value,
                        point: Some(point),
                        size,
                        iteration: point.iteration,
                        origin,
                        generate: self.last_gen,
                    };
                    let (tested, result) = self.test_input(&input, false);
                    tested.failed().then_some((input, result))
                },
                Err(Skipped::Discarded(reason)) => {
                    self.observe_generation_discard(point, origin, &reason);
                    None
                },
                Err(Skipped::Panicked(_)) => None,
            };
            self.report.replay = Some(ReplayOutcome { point, reproduced: failed.is_some() });
            if let Some((input, result)) = failed {
                self.pending = Some((
                    Found {
                        value: input.value,
                        point: Some(point),
                        size: input.size,
                        iteration: point.iteration,
                        result,
                    },
                    None,
                ));
                return true;
            }
        }

        #[cfg(feature = "json")]
        if self.config.regressions.replay {
            let entries = crate::regressions::load(&self.config.regressions.dir, &self.name);
            if !entries.is_empty() {
                self.report.regressions.file =
                    Some(crate::regressions::path_for(&self.config.regressions.dir, &self.name));
            }
            let limit = self.config.report.truncate_debug_at;
            let max_size = self.config.max_size;
            for entry in entries {
                let point = ReplayPoint { seed: entry.seed, iteration: entry.iteration };
                self.report.regressions.replayed += 1;
                let Some(value) = self.regenerate(point) else {
                    self.report.regressions.stale += 1;
                    self.report.warn("regressions", format!("stored input {point} could not be regenerated (discarded or the generator panicked)"));
                    continue;
                };
                let (debug, _, _) = render(&value, limit);
                if debug != entry.debug {
                    self.report.regressions.stale += 1;
                    self.report.warn(
                        "regressions",
                        format!(
                            "stored input {point} no longer matches the generator (stored {}, regenerated {debug}); \
                             delete the entry or rerun the property",
                            entry.debug
                        ),
                    );
                    continue;
                }
                let size = size_for(point.seed, point.iteration, max_size);
                let (result, observed, elapsed) = self.evaluate(value.clone());
                self.observe(ObservationLine {
                    origin: "regression",
                    seed: point.seed,
                    iteration: point.iteration,
                    size,
                    representation: debug.clone(),
                    result: &result,
                    observed: Some(&observed),
                    generate: std::time::Duration::ZERO,
                    execute: elapsed,
                    coverage: None,
                });
                if Signature::of(&result).is_some() {
                    self.pending = Some((
                        Found {
                            value,
                            point: Some(point),
                            size,
                            iteration: point.iteration,
                            result,
                        },
                        None,
                    ));
                    return true;
                }
            }
        }
        false
    }

    /// Fold a worker's statistics into this run.
    fn absorb(&mut self, mut other: Run<G, P>) {
        let report = &mut self.report;
        report.tests += other.report.tests;
        report.discarded += other.report.discarded;
        report.iterations += other.report.iterations;
        report.gen_time += other.report.gen_time;
        report.prop_time += other.report.prop_time;
        for (label, count) in other.report.labels {
            *report.labels.entry(label).or_default() += count;
        }
        for (size, count) in other.report.sizes {
            *report.sizes.entry(size).or_default() += count;
        }
        for (reason, count) in other.report.discard_reasons {
            *report.discard_reasons.entry(reason).or_default() += count;
        }
        report.health.append(&mut other.report.health);
        report.stopped_by_budget |= other.report.stopped_by_budget;
        for (label, (met, total)) in other.covers {
            let entry = self.covers.entry(label).or_insert((0, 0));
            entry.0 += met;
            entry.1 += total;
        }
        if other.report.status != Status::Passed && report.status == Status::Passed {
            report.status = other.report.status;
            report.abort = other.report.abort.take();
        }
        self.seen.extend(other.seen);
        self.generated += other.generated;
        for seed in other.pool.drain() {
            self.pool.add(seed.value, seed.score);
        }
        self.saw_score |= other.saw_score;
        self.stats.passed_seen.extend(other.stats.passed_seen);
        for (name, mut values) in other.stats.features {
            self.stats.features.entry(name).or_default().append(&mut values);
        }
        self.stats.gen_micros.append(&mut other.stats.gen_micros);
        self.stats.prop_micros.append(&mut other.stats.prop_micros);
        if let Some((micros, source)) = other.stats.slowest.take() {
            self.stats.slower(std::time::Duration::from_micros(u64::from(micros)), || source);
        }
        for c in other.stats.samples.drain() {
            self.stats.samples.consider(c.size, c.iteration, c.origin, c.r, || c.source);
        }
        #[cfg(feature = "json")]
        if let (Some(mine), Some(theirs)) = (self.observations.as_mut(), other.observations.take())
        {
            mine.lines.extend(theirs.lines);
        }
    }

    /// Render an input kept by provenance; `None` when it can no longer be
    /// regenerated.
    fn render_provenance(&mut self, source: Provenance) -> Option<String> {
        let limit = self.config.report.truncate_debug_at;
        match source {
            Provenance::Debug(debug) => Some(debug),
            Provenance::Point(point) => self.regenerate(point).map(|value| render(&value, limit).0),
        }
    }

    /// The targeted phase: mutate the best-scoring inputs to push the score
    /// up, looking for a failure along the way.
    fn targeted_search(&mut self) {
        if self.pool.is_empty() {
            return;
        }
        let target_tests = self.config.target.tests;
        let explore = self.config.target.explore;
        let limit = self.config.report.truncate_debug_at;
        let timeout = self.config.timeout_duration();
        let max_discards = (self.config.max_discard_ratio * target_tests as f64).max(0.0) as u64;
        let mut iterations = 0u64;
        let mut discarded = 0u64;

        while iterations < target_tests {
            if let Some(timeout) = timeout
                && self.started.elapsed() > timeout
            {
                self.report.status = Status::TimedOut;
                break;
            }
            if discarded > max_discards {
                self.report.status = Status::GaveUp;
                break;
            }
            iterations += 1;
            let iteration = self.next_iteration();
            let input = if u01(&mut self.rng) < explore {
                let point = ReplayPoint { seed: self.seed, iteration };
                match self.draw_at(point, crate::report::Origin::Explore) {
                    Some(input) => input,
                    None if self.report.status != Status::Passed => break,
                    None => {
                        discarded += 1;
                        continue;
                    },
                }
            } else {
                let Some(seed) = self.pool.pick(&mut self.rng) else { break };
                match self.mutate_at(&seed.value, iteration) {
                    Some(input) => input,
                    None if self.report.status != Status::Passed => break,
                    None => continue,
                }
            };
            let tested = self.test(&input);
            if tested.failed() {
                break;
            }
            if tested.discarded() {
                discarded += 1;
                continue;
            }
            self.keep(input.value, &tested);
        }

        if let Some((best_score, best)) = self.pool.best.clone() {
            self.report.target = Some(TargetSummary {
                scored: self.saw_score,
                iterations,
                improvements: self.pool.improvements,
                best_score,
                best_debug: render(&best, limit).0,
            });
        }
    }

    /// Handle the pending failure: check it reproduces, shrink it (unless
    /// the loop already did), remember it, and fill in the report.
    fn fail(&mut self) {
        let limit = self.config.report.truncate_debug_at;
        let could_shrink = self.reproduce_pending();
        let Some((found, thread)) = self.pending.take() else {
            return;
        };
        let Some(mut shrinking) = self.shrinking.take() else {
            return;
        };
        let (kind, mut message, mut location) = details(found.result);
        let (debug, truncated, _) = render(&found.value, limit);
        let flaky = shrinking.flaky;

        if could_shrink
            && self.config.shrink.enabled
            && !shrinking.done
            && shrinking.stats.steps == 0
        {
            self.drive_shrink(&mut shrinking, found.iteration, found.size);
        }
        let mut shrunk = None;
        let mut shrink_stats = None;
        let minimal = shrinking.minimal.clone();
        if shrinking.done || shrinking.stats.steps > 0 {
            if let Some((shrunk_message, shrunk_location)) = shrinking.failure {
                let (debug, truncated, _) = render(&minimal, limit);
                shrunk = Some(Counterexample {
                    debug,
                    truncated,
                    size: found.size,
                    iteration: found.iteration,
                    thread,
                });
                message = shrunk_message;
                location = shrunk_location;
            }
            shrink_stats = Some(shrinking.stats);
        }
        #[cfg(feature = "coverage")]
        if self.coverage.spectrum.is_some() && !flaky {
            // Passing and failing neighbours of the minimal input sharpen the
            // spectrum where it matters.
            let neighbourhood = mix(self.seed, u64::MAX - 3);
            for i in 0..self.config.locate.neighbourhood as u64 {
                let mut source = Source::new(mix(neighbourhood, i), found.size);
                if let Some(mutant) = self.generator.mutate(&minimal, &mut source) {
                    let observing = self.observations.is_some();
                    let representation =
                        if observing { render(&mutant, usize::MAX).0 } else { String::new() };
                    let before = self.coverage.begin();
                    let (result, observed, elapsed) = self.evaluate(mutant);
                    let covered = self.coverage.end(before, &result, observing);
                    if observing {
                        self.observe(ObservationLine {
                            origin: "neighbourhood",
                            seed: self.seed,
                            iteration: found.iteration,
                            size: found.size,
                            representation,
                            result: &result,
                            observed: Some(&observed),
                            generate: std::time::Duration::ZERO,
                            execute: elapsed,
                            coverage: covered.lines,
                        });
                    }
                }
            }
            let panic = location.as_ref().map(|l| (l.file.as_str(), l.line));
            self.report.locate =
                self.coverage.spectrum.as_ref().map(|s| s.rank(panic, self.config.locate.top));
        }
        // Like Hypothesis's "minimal failing case", the observation file ends
        // with the shrunk input, run once more so Tyche lists it first.
        if self.observations.is_some() && !flaky {
            let representation = render(&minimal, usize::MAX).0;
            let before = self.coverage.begin();
            let (result, observed, elapsed) = self.evaluate(minimal.clone());
            let covered = self.coverage.end(before, &result, true);
            self.observe(ObservationLine {
                origin: "minimal",
                seed: self.seed,
                iteration: found.iteration,
                size: found.size,
                representation,
                result: &result,
                observed: Some(&observed),
                generate: std::time::Duration::ZERO,
                execute: elapsed,
                coverage: covered.lines,
            });
        }

        #[cfg(feature = "json")]
        if self.config.regressions.save
            && !flaky
            && let Some(point) = found.point
        {
            let entry = crate::regressions::Entry::new(
                &self.name,
                point.seed,
                point.iteration,
                debug.clone(),
                shrunk.as_ref().map(|c| c.debug.clone()),
            );
            match crate::regressions::save(&self.config.regressions.dir, &entry) {
                Ok((path, appended)) => {
                    self.report.regressions.file = Some(path);
                    self.report.regressions.saved = appended;
                },
                Err(e) => {
                    self.report.warn(
                        "regressions",
                        format!("could not write {}: {e}", self.config.regressions.dir.display()),
                    )
                },
            }
        }

        self.report.status = Status::Failed;
        self.report.failure = Some(Failure {
            kind,
            message,
            location,
            original: Counterexample {
                debug,
                truncated,
                size: found.size,
                iteration: found.iteration,
                thread,
            },
            shrunk,
            replay: found.point,
            flaky,
            shrink: shrink_stats,
        });
    }

    /// Shrink, localize and remember the failure if there was one, then
    /// the final statistics, health checks and report output (JSON,
    /// observations, profile, the coverage grid).
    pub fn finish(mut self) -> Report {
        if self.pending.is_some() {
            self.fail();
        }
        if self.report.target.is_none()
            && self.mutations > 0
            && let Some((best_score, best)) = self.pool.best.clone()
        {
            self.report.target = Some(TargetSummary {
                scored: self.saw_score,
                iterations: self.mutations,
                improvements: self.pool.improvements,
                best_score,
                best_debug: render(&best, self.config.report.truncate_debug_at).0,
            });
        }
        self.report.duration = self.started.elapsed();
        self.report.distinct_inputs_ratio =
            if self.generated == 0 { 1.0 } else { self.seen.len() as f64 / self.generated as f64 };
        self.report.unique_tests = self.stats.passed_seen.len() as u64;
        self.report.distinct_growth = std::mem::take(&mut self.stats.distinct_growth);
        for (name, values) in std::mem::take(&mut self.stats.features) {
            if !values.is_empty() {
                self.report.features.insert(name, crate::report::FeatureSummary::of(values));
            }
        }
        // Quantiles before any regeneration below adds to the timings.
        let prop = crate::report::DurationStats::of(std::mem::take(&mut self.stats.prop_micros));
        let generation =
            crate::report::DurationStats::of(std::mem::take(&mut self.stats.gen_micros));
        if let Some(prop) = prop {
            let zero = std::time::Duration::ZERO;
            let generation = generation.unwrap_or(crate::report::DurationStats {
                median: zero,
                p90: zero,
                max: zero,
            });
            let slowest =
                self.stats.slowest.take().and_then(|(_, source)| self.render_provenance(source));
            self.report.timing = Some(crate::report::Timing { prop, generation, slowest });
        }
        for candidate in self.stats.samples.drain() {
            if let Some(debug) = self.render_provenance(candidate.source) {
                self.report.samples.push(crate::report::Sample {
                    debug: cap(debug, SAMPLE_CHARS),
                    origin: candidate.origin,
                    size: candidate.size,
                    iteration: candidate.iteration,
                });
            }
        }
        for (label, (met, total)) in &self.covers {
            let share = if *total == 0 { 0.0 } else { *met as f64 / *total as f64 };
            self.report.covers.insert(label.clone(), share);
            if *total >= 20 && share < self.config.coverage.min_cover {
                self.report.warn(
                    "cover",
                    format!(
                        "cover `{label}` was met by {:.1}% of passing tests, below the required {:.0}%",
                        share * 100.0,
                        self.config.coverage.min_cover * 100.0
                    ),
                );
            }
        }
        #[cfg(feature = "coverage")]
        {
            if let Some(tracker) = self.coverage.tracker.take() {
                let mut summary = tracker.summary();
                if let Some(targets) = self.coverage.targets.take() {
                    summary.targets = targets.outcomes();
                    for outcome in &summary.targets {
                        if outcome.functions.is_empty() || outcome.hits > 0 {
                            continue;
                        }
                        let function = outcome.functions.join(", ");
                        let how = if outcome.function_inputs == 0 {
                            format!("`{function}` was never entered")
                        } else {
                            format!(
                                "`{function}` was entered by {} inputs that took other branches",
                                outcome.function_inputs
                            )
                        };
                        self.report.warn(
                            "coverage_target_missed",
                            format!("target {} was never reached: {how}", outcome.target),
                        );
                    }
                }
                if summary.inputs >= 100 && !summary.partial.is_empty() {
                    let since = summary.inputs - 1 - summary.last_novelty_at;
                    let missed: usize =
                        summary.partial.iter().map(|(_, hit, total)| total - hit).sum();
                    if since > (summary.inputs / 4).max(100) {
                        self.report.warn(
                            "coverage_plateau",
                            format!(
                                "coverage of `{}` last grew at input {} and {missed} {} in the functions it \
                                 reaches never hit; the generator may be too narrow",
                                summary.filter,
                                summary.last_novelty_at,
                                if missed == 1 { "counter was" } else { "counters were" }
                            ),
                        );
                    }
                }
                #[cfg(feature = "json")]
                if let (Some(grid), Some(dir)) =
                    (self.coverage.grid.take(), self.config.coverage.html.as_ref())
                {
                    let samples: Vec<(String, String, usize)> = self
                        .report
                        .samples
                        .iter()
                        .map(|s| (s.debug.clone(), s.origin.to_string(), s.size))
                        .collect();
                    let html = grid.render(
                        &crate::coverage::grid::Facts {
                            property: &self.name,
                            seed: self.seed,
                            tests: self.report.tests,
                            filter: &summary.filter,
                            property_type: self.coverage.property_type,
                        },
                        &summary.targets,
                        &samples,
                    );
                    match grid.write(dir, &self.name, &html) {
                        Ok(path) => self.report.coverage_html = Some(path),
                        Err(e) => {
                            self.report.warn(
                                "coverage_html",
                                format!(
                                    "could not write the coverage grid under {}: {e}",
                                    dir.display()
                                ),
                            )
                        },
                    }
                }
                self.report.coverage = Some(summary);
            }
            if let Some(dir) = &self.config.coverage.profraw_dir
                && crate::coverage::available()
            {
                match crate::coverage::tracker::write_profraw(dir, &self.name) {
                    Ok(path) => self.report.profraw = Some(path),
                    Err(e) => {
                        self.report.warn(
                            "profraw",
                            format!("could not write profraw to {}: {e}", dir.display()),
                        )
                    },
                }
            }
        }
        health::assess(&mut self.report, &self.config);
        #[cfg(feature = "json")]
        if let Some(path) = &self.config.report.json_path {
            use std::io::Write;
            let line = serde_json::to_string(&self.report).expect("Report is always serialisable");
            let written = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .and_then(|mut file| writeln!(file, "{line}"));
            if let Err(e) = written {
                self.report
                    .warn("report_json", format!("could not append to {}: {e}", path.display()));
            }
        }
        #[cfg(feature = "json")]
        if let (Some(mut observations), Some(dir)) =
            (self.observations.take(), self.config.report.observations.as_ref())
        {
            observations.info("crabcheck report", &format!("```\n{}```", self.report));
            if let Err(e) = observations.write(dir) {
                self.report.warn(
                    "observations",
                    format!("could not write the observation file under {}: {e}", dir.display()),
                );
            }
        }
        self.report
    }
}

/// Identifies "the same failure" while shrinking: the kind, and for panics
/// the site. Messages are ignored because they usually embed the input.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Signature {
    kind: FailureKind,
    panic_site: Option<(String, u32)>,
}

impl Signature {
    fn of(result: &Result<Outcome, PanicInfo>) -> Option<Self> {
        match result {
            Ok(Outcome::Pass) | Ok(Outcome::Discard(_)) => None,
            Ok(Outcome::False) => Some(Signature { kind: FailureKind::False, panic_site: None }),
            Ok(Outcome::Fail(_)) => Some(Signature { kind: FailureKind::Error, panic_site: None }),
            Err(info) => {
                Some(Signature {
                    kind: FailureKind::Panic,
                    panic_site: info.location.as_ref().map(|l| (l.file.clone(), l.line)),
                })
            },
        }
    }
}

/// Kind, message and location of a failing result.
fn details(result: Result<Outcome, PanicInfo>) -> (FailureKind, Option<String>, Option<Location>) {
    match result {
        Ok(Outcome::Fail(message)) => (FailureKind::Error, Some(message), None),
        Err(info) => (FailureKind::Panic, Some(info.message), info.location),
        Ok(_) => (FailureKind::False, None, None),
    }
}

/// Run the property once with panic capture and the side channel active.
fn evaluate<T, R: Testable, P: FnMut(T) -> R>(
    property: &mut P,
    value: T,
) -> (Result<Outcome, PanicInfo>, labels::Observed) {
    labels::collect(|| panic::catch(|| property(value).into_outcome()))
}

/// `Debug`-render `value`, truncating at `limit` characters. Returns the
/// (possibly truncated) string, whether it was truncated, and the full
/// length in bytes.
fn render<T: Debug>(value: &T, limit: usize) -> (String, bool, usize) {
    let mut debug = format!("{value:?}");
    let full_len = debug.len();
    if debug.chars().count() > limit {
        let cut = debug.char_indices().nth(limit).map_or(debug.len(), |(i, _)| i);
        debug.truncate(cut);
        debug.push('…');
        (debug, true, full_len)
    } else {
        (debug, false, full_len)
    }
}

/// Hash a value's `Debug` rendering without allocating it.
fn debug_hash<T: Debug>(value: &T) -> u64 {
    struct HashWriter(DefaultHasher);
    impl fmt::Write for HashWriter {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            self.0.write(s.as_bytes());
            Ok(())
        }
    }
    let mut writer = HashWriter(DefaultHasher::new());
    let _ = write!(writer, "{value:?}");
    writer.0.finish()
}

/// The size parameter for generation attempt `iteration`, a pure function of
/// the run seed and the iteration so an input can be regenerated from those
/// two numbers: `max_size` one time in sixteen, otherwise
/// `floor(max_size * u^2)` for `u` uniform in `[0, 1)`.
fn size_for(seed: u64, iteration: u64, max_size: usize) -> usize {
    let mut rng = ChaCha8Rng::seed_from_u64(mix(seed, iteration) ^ 0x5A5A_5A5A_5A5A_5A5A);
    if uniform(&mut rng, 15) == 0 {
        max_size
    } else {
        let u = u01(&mut rng);
        (max_size as f64 * u * u).floor() as usize
    }
}

/// A fresh seed from the operating system's randomness (via `RandomState`).
fn random_seed() -> u64 {
    RandomState::new().hash_one(Instant::now())
}

/// Derive an independent 64-bit value from `seed` and `index` (SplitMix64).
pub(crate) fn mix(seed: u64, index: u64) -> u64 {
    let mut z = seed
        .wrapping_add(index.wrapping_mul(0x9E37_79B9_7F4A_7C15))
        .wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use {
        super::*,
        crate::{
            GenExt,
            classify,
            gens,
        },
    };

    fn cfg() -> Config {
        Config::default().seed(1).tests(64).no_regressions()
    }

    #[test]
    fn passing_property() {
        let report = check(&cfg(), gens::vec(gens::int(0..=9)), |v: Vec<i32>| v.len() <= 100);
        assert_eq!(report.status, Status::Passed);
        assert_eq!(report.tests, 64);
        assert_eq!(report.discarded, 0);
        assert_eq!(report.iterations, 64);
        assert!(*report.sizes.keys().max().unwrap() >= 50, "{:?}", report.sizes);
        assert!(*report.sizes.keys().min().unwrap() < 10, "{:?}", report.sizes);
        assert!(report.passed());
        assert!(report.distinct_inputs_ratio >= 0.5, "{}", report.distinct_inputs_ratio);
        assert!(report.health.is_empty(), "{:?}", report.health);
        report.assert_passed();
        assert!(report.to_string().starts_with("crabcheck: ok, 64 tests passed"));
    }

    #[test]
    fn false_result_is_a_shrunk_counterexample() {
        let report = check(&cfg(), gens::vec(gens::int(0..=9)), |v: Vec<i32>| v.len() < 3);
        assert_eq!(report.status, Status::Failed);
        let failure = report.failure.as_ref().unwrap();
        assert_eq!(failure.kind, FailureKind::False);
        assert!(failure.original.debug.starts_with('['));
        assert!(!failure.flaky);
        assert_eq!(failure.original.iteration + 1, report.iterations);
        assert_eq!(failure.counterexample().debug, "[0, 0, 0]");
        assert!(failure.shrink.as_ref().unwrap().accepted > 0);
        let point = failure.replay.unwrap();
        assert_eq!(point.seed, 1);
        let text = report.to_string();
        assert!(text.contains("property returned false"), "{text}");
        assert!(text.contains("counterexample: [0, 0, 0]"), "{text}");
        assert!(text.contains(&format!("CRABCHECK_REPLAY={point}")), "{text}");
    }

    #[test]
    fn replay_point_reproduces_without_random_search() {
        let property = |v: Vec<i32>| v.len() < 3;
        let first = check(&cfg(), gens::vec(gens::int(0..=9)), property);
        let point = first.failure.as_ref().unwrap().replay.unwrap();

        let second = check(&cfg().seed(999).replay(point), gens::vec(gens::int(0..=9)), property);
        assert_eq!(second.status, Status::Failed);
        assert_eq!(second.iterations, 0, "no random search was needed");
        assert_eq!(
            second.failure.as_ref().unwrap().original,
            first.failure.as_ref().unwrap().original
        );
        assert!(second.replay.as_ref().unwrap().reproduced);
        assert!(second.to_string().contains("reproduced the failure"));

        let fixed = check(&cfg().replay(point), gens::vec(gens::int(0..=9)), |_: Vec<i32>| true);
        assert_eq!(fixed.status, Status::Passed);
        assert!(!fixed.replay.as_ref().unwrap().reproduced);
        assert_eq!(fixed.tests, 64);
    }

    #[cfg(feature = "json")]
    #[test]
    fn regressions_are_saved_and_replayed() {
        let dir = std::env::temp_dir()
            .join(format!("crabcheck-runner-regressions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let config = Config::default().seed(5).tests(64).name("sum_prop").regressions_dir(&dir);
        let property = |v: Vec<i32>| v.iter().sum::<i32>() < 10;

        let first = check(&config, gens::vec(gens::int(0..=9)), property);
        assert_eq!(first.status, Status::Failed);
        assert!(first.regressions.saved);
        let file = first.regressions.file.clone().unwrap();
        assert!(file.ends_with("sum_prop.jsonl"));
        assert!(first.to_string().contains("remembered in"));

        let second = check(&config.clone().seed(6), gens::vec(gens::int(0..=9)), property);
        assert_eq!(second.status, Status::Failed);
        assert_eq!(second.regressions.replayed, 1);
        assert_eq!(second.iterations, 0);
        assert_eq!(second.counterexample().unwrap().debug, first.counterexample().unwrap().debug);
        assert!(!second.regressions.saved, "the same point is not stored twice");

        let fixed = check(&config, gens::vec(gens::int(0..=9)), |_: Vec<i32>| true);
        assert_eq!(fixed.status, Status::Passed);
        assert_eq!(fixed.regressions.replayed, 1);

        let changed = check(&config, gens::vec(gens::int(100..=109)), |_: Vec<i32>| true);
        assert_eq!(changed.regressions.stale, 1);
        assert!(changed.health.iter().any(|w| w.kind == "regressions"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(feature = "json")]
    #[test]
    fn json_report_file_is_appended() {
        let dir =
            std::env::temp_dir().join(format!("crabcheck-runner-json-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("reports.jsonl");
        let mut config = cfg();
        config.report.json_path = Some(path.clone());
        check(&config, gens::int(0..=9), |_: i32| true);
        check(&config, gens::int(0..=9), |x: i32| x < 5);
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<serde_json::Value> =
            text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["status"], "Passed");
        assert_eq!(lines[1]["failure"]["shrunk"]["debug"], "5");
        assert!(lines[1]["failure"]["replay"]["iteration"].is_number());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn env_problems_become_warnings() {
        let (config, problems) =
            cfg().apply_env_vars(vec![("CRABCHECK_BOGUS".to_string(), "1".to_string())]);
        assert_eq!(problems.len(), 1);
        let report = check(&config, gens::int(0..=9), |_: i32| true);
        assert!(report.passed());
    }

    #[test]
    fn health_warnings_for_discards_and_degenerate_generators() {
        let report = check(&cfg(), gens::int(0..=9), |x: i32| {
            crate::assume!(x == 0);
            TestResult::Pass
        });
        assert!(report.health.iter().any(|w| w.kind == "discard_ratio"), "{:?}", report.health);
        let report = check(&cfg(), gens::constant(1), |_: i32| true);
        assert!(report.distinct_inputs_ratio < 0.1);
        assert!(
            report.health.iter().any(|w| w.kind == "low_distinct_inputs"),
            "{:?}",
            report.health
        );
    }

    #[test]
    fn error_and_test_result_failures_carry_messages() {
        let report = check(&cfg(), gens::int(0..=9), |x: i32| {
            if x > 5 { Err(format!("too big: {x}")) } else { Ok(()) }
        });
        let failure = report.failure.as_ref().unwrap();
        assert_eq!(failure.kind, FailureKind::Error);
        assert_eq!(
            failure.message.as_deref(),
            Some("\"too big: 6\""),
            "message reflects the shrunk input"
        );
        assert_eq!(failure.counterexample().debug, "6");

        let report =
            check(&cfg(), gens::int(0..=9), |x: i32| TestResult::check(x < 5, "x must be small"));
        let failure = report.failure.as_ref().unwrap();
        assert_eq!(failure.kind, FailureKind::Error);
        assert_eq!(failure.message.as_deref(), Some("x must be small"));
        assert_eq!(failure.counterexample().debug, "5");
    }

    #[test]
    fn panics_are_captured_with_location() {
        let report = check(&cfg(), gens::int(0..=9), |x: i32| {
            if x > 3 {
                panic!("x was {x}");
            }
        });
        let failure = report.failure.as_ref().unwrap();
        assert_eq!(failure.kind, FailureKind::Panic);
        assert_eq!(failure.message.as_deref(), Some("x was 4"));
        let location = failure.location.as_ref().expect("panic location");
        assert!(location.file.ends_with("runner/mod.rs"), "{}", location.file);
        assert!(report.to_string().contains("property panicked: x was 4"));
    }

    #[test]
    fn shrinking_keeps_the_failure_signature() {
        let report = check(&cfg().seed(3), gens::int(0..=100), |x: i32| {
            if x > 50 {
                panic!("large");
            }
            x <= 10
        });
        let failure = report.failure.as_ref().unwrap();
        match failure.kind {
            FailureKind::Panic => assert_eq!(failure.counterexample().debug, "51"),
            FailureKind::False => assert_eq!(failure.counterexample().debug, "11"),
            FailureKind::Error => unreachable!(),
        }
    }

    #[test]
    fn attached_shrinker_is_used_and_filtered() {
        let generator = gens::int(0i32..=100)
            .filter("even", |x| x % 2 == 0)
            .with_shrink(|x: &i32| vec![x - 2, x / 2]);
        let report = check(&cfg(), generator, |x: i32| x < 11);
        let failure = report.failure.as_ref().unwrap();
        assert_eq!(failure.counterexample().debug, "12", "{report}");
        assert!(failure.shrink.as_ref().unwrap().steps > 0);
    }

    #[test]
    fn shrinking_can_be_disabled() {
        let report =
            check(&cfg().no_shrink(), gens::vec(gens::int(0..=9)), |v: Vec<i32>| v.len() < 3);
        let failure = report.failure.as_ref().unwrap();
        assert!(failure.shrunk.is_none());
        assert!(failure.shrink.is_none());
        assert_eq!(failure.counterexample(), &failure.original);
    }

    #[test]
    fn shrink_budget_is_reported() {
        let config = Config {
            shrink: crate::config::ShrinkConfig { max_steps: 3, ..Default::default() },
            ..cfg()
        };
        let report = check(&config, gens::vec(gens::int(0..=1000)).len(10..=20), |v: Vec<i32>| {
            v.iter().sum::<i32>() < 100
        });
        let failure = report.failure.as_ref().unwrap();
        assert!(failure.shrink.as_ref().unwrap().budget_exhausted);
        assert!(report.health.iter().any(|w| w.kind == "shrink_budget"));
        assert!(report.to_string().contains("budget exhausted"));
    }

    #[test]
    fn discards_from_property_and_generator_are_counted() {
        let report = check(&cfg(), gens::int(0..=9), |x: i32| {
            crate::assume!(x % 2 == 0);
            TestResult::Pass
        });
        assert_eq!(report.status, Status::Passed);
        assert_eq!(report.tests, 64);
        assert!(report.discarded > 0);
        assert_eq!(
            report.discard_reasons.keys().next().map(String::as_str),
            Some("assume: x % 2 == 0")
        );

        let report =
            check(&cfg(), gens::int(0i32..=9).filter("even", |x| x % 2 == 0), |_: i32| true);
        assert_eq!(report.status, Status::Passed);
        assert_eq!(report.discarded, 0, "filter retries do not discard unless exhausted");
        let report = check(
            &cfg().filter_max_attempts(2),
            gens::int(0i32..=9).filter("never", |_| false),
            |_: i32| true,
        );
        assert_eq!(report.status, Status::GaveUp);
        assert_eq!(report.discard_reasons.get("filter:never").copied(), Some(report.discarded));
    }

    #[test]
    fn gives_up_when_everything_is_discarded() {
        let report = check(&cfg(), gens::int(0..=9), |_: i32| TestResult::discard("nope"));
        assert_eq!(report.status, Status::GaveUp);
        assert_eq!(report.tests, 0);
        assert!(report.discarded > 64 * 10);
        assert!(report.to_string().starts_with("crabcheck: GAVE UP"));
    }

    #[test]
    fn times_out() {
        let config = cfg().tests(1_000_000).timeout(std::time::Duration::from_millis(20));
        let report = check(&config, gens::int(0..=9), |_: i32| {
            std::thread::sleep(std::time::Duration::from_millis(1));
            true
        });
        assert_eq!(report.status, Status::TimedOut);
        assert!(report.tests < 1_000_000);
    }

    #[test]
    fn time_budget_stops_the_random_phase() {
        let config = cfg().tests(1_000_000).budget(std::time::Duration::from_millis(20));
        let report = check(&config, gens::int(0..=9), |_: i32| {
            std::thread::sleep(std::time::Duration::from_millis(1));
            true
        });
        assert_eq!(report.status, Status::Passed);
        assert!(report.stopped_by_budget);
        assert!(report.tests < 1_000_000);
        assert!(report.to_string().contains("stopped by the time budget"));
    }

    #[test]
    fn generator_panic_aborts() {
        let report =
            check(&cfg(), |_: &mut Source| -> i32 { panic!("generator bug") }, |_: i32| true);
        assert_eq!(report.status, Status::Aborted);
        let abort = report.abort.as_ref().unwrap();
        assert_eq!(abort.message, "generator bug");
        assert!(abort.location.is_some());
        assert!(report.to_string().contains("ABORTED"));
    }

    #[test]
    fn labels_are_reported_for_passing_tests() {
        let report = check(&cfg(), gens::vec(gens::int(0..=9)), |v: Vec<i32>| {
            classify(v.is_empty(), "empty");
            classify(!v.is_empty(), "non-empty");
            true
        });
        let empty = report.labels.get("empty").copied().unwrap_or(0);
        let non_empty = report.labels.get("non-empty").copied().unwrap_or(0);
        assert_eq!(empty + non_empty, 64);
        assert!(report.to_string().contains("labels: empty"));
    }

    #[test]
    fn same_seed_same_report() {
        let run =
            || check(&cfg(), gens::vec(gens::int(0..=9)), |v: Vec<i32>| v.iter().sum::<i32>() < 20);
        let a = run();
        let b = run();
        assert_eq!(a.status, Status::Failed);
        let fa = a.failure.as_ref().unwrap();
        let fb = b.failure.as_ref().unwrap();
        assert_eq!(fa.original, fb.original);
        assert_eq!(fa.shrunk, fb.shrunk);
        assert_eq!(fa.replay, fb.replay);
        assert_eq!(a.tests, b.tests);
        let c = check(&cfg().seed(2), gens::vec(gens::int(0..=9)), |v: Vec<i32>| {
            v.iter().sum::<i32>() < 20
        });
        assert_ne!(a.seed, c.seed);
    }

    #[test]
    fn unseeded_runs_get_distinct_seeds() {
        let a = check(&Config::default().tests(1).no_regressions(), gens::bool(), |_| true);
        let b = check(&Config::default().tests(1).no_regressions(), gens::bool(), |_| true);
        assert_ne!(a.seed, b.seed);
    }

    #[test]
    fn flaky_property_is_flagged_and_not_shrunk() {
        let calls = Cell::new(0);
        let report = check(&cfg(), gens::int(0..=9), |_: i32| {
            calls.set(calls.get() + 1);
            calls.get() != 3
        });
        let failure = report.failure.as_ref().unwrap();
        assert!(failure.flaky);
        assert!(failure.shrunk.is_none());
        assert_eq!(report.health[0].kind, "flaky");
        assert!(report.to_string().contains("FLAKY"));
    }

    #[test]
    fn debug_output_is_truncated() {
        let config = Config {
            report: crate::config::ReportConfig { truncate_debug_at: 10, ..Default::default() },
            ..cfg().no_shrink()
        };
        let report = check(&config, gens::vec(gens::int(0..=9)).len(50..=50), |_: Vec<i32>| false);
        let original = &report.failure.as_ref().unwrap().original;
        assert!(original.truncated);
        assert_eq!(original.debug.chars().count(), 11);
        assert!(report.to_string().contains("(truncated)"));
    }

    #[test]
    fn concurrent_checks_keep_their_own_panics() {
        let handles: Vec<_> = (0..6)
            .map(|i| {
                std::thread::spawn(move || {
                    let report = check(&cfg(), gens::int(0..=9), move |x: i32| {
                        if x > 2 {
                            panic!("worker {i}");
                        }
                    });
                    let failure = report.failure.unwrap();
                    assert_eq!(failure.message.as_deref(), Some(format!("worker {i}").as_str()));
                    assert_eq!(failure.counterexample().debug, "3");
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
    }

    #[test]
    fn targeted_search_finds_what_random_search_misses() {
        let generator = || gens::vec(gens::int(0u8..=255)).len(8..=8);
        let property = |v: Vec<u8>| {
            let sum: u32 = v.iter().map(|&x| u32::from(x)).sum();
            crate::target(f64::from(sum));
            sum < 1900
        };
        let mut random_only = cfg().seed(11);
        random_only.target.tests = 0;
        let report = check(&random_only, generator(), property);
        assert_eq!(report.status, Status::Passed, "{report}");
        let summary = report.target.as_ref().expect("scores were observed");
        assert_eq!(summary.iterations, 0);
        assert!(summary.best_score < 1900.0);

        let mut targeted = cfg().seed(11);
        targeted.target.tests = 2000;
        let report = check(&targeted, generator(), property);
        assert_eq!(report.status, Status::Failed, "{report}");
        let failure = report.failure.as_ref().unwrap();
        assert!(failure.replay.is_none(), "mutants replay through the run seed");
        let shrunk: Vec<u32> = failure
            .counterexample()
            .debug
            .trim_matches(|c| c == '[' || c == ']')
            .split(", ")
            .map(|x| x.parse().unwrap())
            .collect();
        assert_eq!(shrunk.iter().sum::<u32>(), 1900, "{shrunk:?}");
        assert!(report.to_string().contains("found in the targeted phase"));

        let again = check(&targeted, generator(), property);
        assert_eq!(again.failure.as_ref().unwrap().original, failure.original, "deterministic");
    }

    #[test]
    fn target_summary_reports_the_best_input() {
        let generator = gens::vec(gens::int(0..=9)).len(4..=4);
        let property = |v: Vec<i32>| {
            crate::target(f64::from(v.iter().sum::<i32>()));
            true
        };
        let mut random_only = cfg().seed(4);
        random_only.target.tests = 0;
        let baseline = check(&random_only, generator.clone(), property);
        let baseline_best = baseline.target.as_ref().unwrap().best_score;

        let mut config = cfg().seed(4);
        config.target.tests = 300;
        let report = check(&config, generator.clone(), property);
        assert!(report.passed());
        let summary = report.target.as_ref().unwrap();
        assert!(summary.best_score > baseline_best, "{} vs {baseline_best}", summary.best_score);
        assert!(summary.best_score >= 30.0, "{}", summary.best_score);
        assert_eq!(summary.best_debug.matches(',').count(), 3, "{}", summary.best_debug);
        assert_eq!(summary.iterations, 300);
        assert!(summary.improvements > 0);
        assert!(report.to_string().contains("target: best score"));

        let plain = check(&config, generator, |_: Vec<i32>| true);
        assert!(plain.target.is_none(), "no scores, no targeted phase");
        assert_eq!(plain.iterations, 64);
    }

    #[test]
    fn parallel_runs_merge_statistics_and_find_failures() {
        let config = cfg().threads(4).tests(100);
        let report = check_par(&config, gens::vec(gens::int(0..=9)), |v| {
            classify(v.is_empty(), "empty");
            v.len() <= 100
        });
        assert!(report.passed(), "{report}");
        assert_eq!(report.threads, 4);
        assert!(report.tests >= 100, "{}", report.tests);
        assert!(report.labels.contains_key("empty"));

        let report = check_par(&config, gens::vec(gens::int(0..=9)), |v| v.len() < 3);
        assert_eq!(report.status, Status::Failed);
        let failure = report.failure.as_ref().unwrap();
        assert!(failure.original.thread.is_some());
        assert_eq!(
            failure.counterexample().debug,
            "[0, 0, 0]",
            "shrinking runs on the calling thread"
        );
        assert!(report.to_string().contains("found by worker"));
        // The worker's replay point regenerates the same input through `check`.
        let point = failure.replay.unwrap();
        let replayed =
            check(&cfg().replay(point), gens::vec(gens::int(0..=9)), |v: Vec<i32>| v.len() < 3);
        assert_eq!(replayed.failure.as_ref().unwrap().original.debug, failure.original.debug);

        let serial = check_par(&cfg().threads(1), gens::vec(gens::int(0..=9)), |v| v.len() < 3);
        let plain = check(&cfg(), gens::vec(gens::int(0..=9)), |v: Vec<i32>| v.len() < 3);
        assert_eq!(serial.failure.as_ref().unwrap().replay, plain.failure.as_ref().unwrap().replay);
        assert_eq!(serial.threads, 1);
    }

    #[test]
    fn parallel_targeted_phase_runs_after_the_workers() {
        let mut config = cfg().threads(3).tests(60);
        config.target.tests = 50;
        let report = check_par(&config, gens::vec(gens::int(0..=9)).len(4..=4), |v| {
            crate::target(f64::from(v.iter().sum::<i32>()));
            true
        });
        assert!(report.passed());
        let summary = report.target.as_ref().unwrap();
        assert_eq!(summary.iterations, 50);
        assert!(summary.best_score >= 20.0, "{}", summary.best_score);
    }

    #[test]
    fn features_timing_and_samples_are_reported() {
        let mut config = cfg();
        config.tests = 200;
        let report = check(&config, gens::vec(gens::int(0i32..=9)), |v: Vec<i32>| {
            crate::feature("len", v.len());
            crate::feature("parity", if v.len().is_multiple_of(2) { "even" } else { "odd" });
            crate::target(v.iter().sum::<i32>() as f64);
            true
        });
        assert_eq!(report.status, Status::Passed, "{report}");
        let len = &report.features["len"];
        assert_eq!(len.count, report.tests, "the targeted phase's passes count too");
        assert!(len.min <= len.median && len.median <= len.p90 && len.p90 <= len.max);
        assert_eq!(len.histogram.iter().map(|b| b.count).sum::<u64>(), report.tests, "{len:?}");
        assert!(report.features.contains_key("target"));
        assert_eq!(
            report.labels.get("parity=even").copied().unwrap_or(0)
                + report.labels.get("parity=odd").copied().unwrap_or(0),
            report.tests
        );
        assert!(report.unique_tests <= report.tests && report.unique_tests > 100, "{report}");
        assert!(!report.distinct_growth.points.is_empty());
        let timing = report.timing.as_ref().expect("inputs ran");
        assert!(timing.prop.median <= timing.prop.p90 && timing.prop.p90 <= timing.prop.max);
        assert!(timing.slowest.is_some());
        assert!(!report.samples.is_empty() && report.samples.len() <= 5, "{report}");
        let smallest = &report.samples[0];
        assert!(report.samples.iter().all(|s| s.size >= smallest.size));
        let text = report.to_string();
        assert!(text.contains("feature len:") && text.contains("histogram:"), "{text}");
        assert!(text.contains("timing: property median") && text.contains("sample ("), "{text}");
        assert!(text.contains("distinct,"), "{text}");
    }

    #[cfg(feature = "json")]
    #[test]
    fn observation_files_follow_openpbtstats() {
        let dir = std::env::temp_dir().join(format!("crabcheck-observed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut config = cfg().name("observed");
        config.tests = 50;
        config.report.observations = Some(dir.clone());
        let report = check(&config, gens::vec(gens::int(0i32..=9)), |v: Vec<i32>| {
            crate::feature("len", v.len());
            crate::label("seen");
            if v.len() == 7 {
                crate::TestResult::discard("seven")
            } else {
                crate::TestResult::check(v.iter().sum::<i32>() < 40, "sum too big")
            }
        });
        assert_eq!(report.status, Status::Failed, "{report}");
        let text = std::fs::read_to_string(dir.join("observed.jsonl")).expect("file written");
        let _ = std::fs::remove_dir_all(&dir);
        let lines: Vec<serde_json::Value> =
            text.lines().map(|l| serde_json::from_str(l).expect("valid json")).collect();
        let info = lines.last().unwrap();
        assert_eq!(info["type"], "info");
        assert_eq!(info["title"], "crabcheck report");
        assert!(info["content"].as_str().unwrap().contains("crabcheck: FAILED"));
        let cases: Vec<&serde_json::Value> =
            lines.iter().filter(|l| l["type"] == "test_case").collect();
        assert!(
            cases
                .iter()
                .all(|c| c["run_start"] == info["run_start"] && c["property"] == "observed")
        );
        let count = |status: &str, origin: &str| {
            cases.iter().filter(|c| c["status"] == status && c["how_generated"] == origin).count()
                as u64
        };
        assert_eq!(count("passed", "random"), report.tests);
        assert_eq!(count("gave_up", "random"), report.discarded);
        assert_eq!(count("failed", "random"), 1);
        assert!(count("passed", "shrink") + count("failed", "shrink") > 0, "shrink candidates");
        let last_case = cases.last().unwrap();
        assert_eq!(last_case["how_generated"], "minimal");
        assert_eq!(last_case["status"], "failed");
        assert_eq!(
            last_case["representation"],
            report.failure.as_ref().unwrap().counterexample().debug
        );
        let passed = cases.iter().find(|c| c["status"] == "passed").unwrap();
        assert!(passed["representation"].as_str().unwrap().starts_with('['));
        assert!(passed["features"]["len"].is_number());
        assert_eq!(passed["features"]["seen"], "");
        assert!(passed["timing"]["execute:test"].is_number());
        assert!(passed["timing"]["generate:input"].is_number());
        assert_eq!(passed["metadata"]["seed"], report.seed);
        let failed = cases.iter().find(|c| c["status"] == "failed").unwrap();
        assert_eq!(failed["status_reason"], "sum too big");
        let discarded = cases.iter().find(|c| c["status"] == "gave_up").unwrap();
        assert_eq!(discarded["status_reason"], "seven");
        #[cfg(feature = "coverage")]
        if crate::coverage::available() && crate::coverage::locate::Spectrum::new(&[]).is_some() {
            assert!(passed["coverage"].is_object(), "{passed}");
        } else {
            assert!(passed["coverage"].is_null(), "{passed}");
        }
        #[cfg(not(feature = "coverage"))]
        assert!(passed["coverage"].is_null(), "{passed}");
    }

    #[test]
    fn samples_keep_the_smallest_the_largest_and_a_reservoir() {
        let mut samples = Samples::new(5);
        for (i, size) in [5usize, 3, 9, 1, 7, 8, 2, 6].into_iter().enumerate() {
            let r = hash_u01(1, i as u64);
            samples.consider(size, i as u64, crate::report::Origin::Random, r, || {
                Provenance::Debug(size.to_string())
            });
        }
        let kept = samples.drain();
        assert_eq!(kept[0].size, 1);
        assert_eq!(kept[1].size, 9);
        assert!(kept.len() <= 5 && kept.len() >= 3, "{}", kept.len());
        let mut none = Samples::new(0);
        none.consider(
            1,
            0,
            crate::report::Origin::Random,
            0.5,
            || Provenance::Debug(String::new()),
        );
        assert!(none.drain().is_empty());
    }

    #[test]
    fn cover_labels_are_reported_and_warned() {
        let report = check(&cfg(), gens::vec(gens::int(0..=9)), |v: Vec<i32>| {
            crate::cover(v.len() > 100, "long");
            crate::cover(true, "always");
            true
        });
        assert_eq!(report.covers.get("long"), Some(&0.0));
        assert_eq!(report.covers.get("always"), Some(&1.0));
        let warning = report.health.iter().find(|w| w.kind == "cover").expect("cover warning");
        assert!(warning.message.contains("`long`"), "{}", warning.message);
        assert!(report.to_string().contains("cover: always 100%, long 0%"), "{report}");
    }

    #[cfg(feature = "coverage")]
    #[test]
    fn the_default_coverage_filter_is_the_property_crate() {
        assert_eq!(crate_of("bst::main::{{closure}}"), "bst");
        assert_eq!(crate_of("&my_crate::props::sorted"), "my_crate");
        assert_eq!(crate_of("&mut my_crate::props::sorted"), "my_crate");
        assert_eq!(crate_of("crabcheck"), "crabcheck");
        assert_eq!(crate_of(super::crate_of_probe_name()), "crabcheck");
    }

    #[cfg(all(crabcheck_instrumented, feature = "coverage"))]
    const BRANCHY_LINE: u32 = line!() + 3;
    #[cfg(all(crabcheck_instrumented, feature = "coverage"))]
    #[rustfmt::skip]
    fn branchy(x: i32) -> i32 {
        if x > 1000 {
            1
        } else {
            2
        }
    }

    #[cfg(all(crabcheck_instrumented, feature = "coverage"))]
    #[test]
    fn coverage_targets_are_resolved_tracked_and_reported() {
        use crate::config::CoverageTarget;
        let then_branch = CoverageTarget::new(file!(), BRANCHY_LINE + 2..=BRANCHY_LINE + 2);
        let else_branch = CoverageTarget::new(file!(), BRANCHY_LINE + 4..=BRANCHY_LINE + 4);
        let nowhere = CoverageTarget::new("no/such/file.rs", 1..=1);
        let mut config = cfg();
        config.tests = 100;
        config.coverage.targets = vec![then_branch.clone(), else_branch.clone(), nowhere.clone()];
        config.coverage.targets_file = None;
        let report = check(&config, gens::int(-100i32..=100), |x| branchy(x) == 2);
        assert_eq!(report.status, Status::Passed, "{report}");
        let coverage = report.coverage.as_ref().expect("instrumented");
        assert_eq!(coverage.targets.len(), 3, "{report}");
        let (then, els, none) = (&coverage.targets[0], &coverage.targets[1], &coverage.targets[2]);
        assert_eq!(then.target, then_branch);
        assert!(then.functions.iter().any(|f| f.contains("branchy")), "{then:?}");
        assert_eq!(then.hits, 0, "{then:?}");
        assert!(then.function_inputs > 0, "{then:?}");
        assert_eq!(els.hits, els.function_inputs, "{els:?}");
        assert_eq!(els.first_hit_at, Some(1), "{els:?}");
        assert!(none.functions.is_empty() && none.target == nowhere, "{none:?}");
        let kinds: Vec<&str> = report.health.iter().map(|w| w.kind.as_str()).collect();
        assert!(kinds.contains(&"coverage_target_missed"), "{kinds:?}");
        assert!(kinds.contains(&"coverage_target"), "{kinds:?}");
        let text = report.to_string();
        assert!(text.contains("never reached") && text.contains("took other branches"), "{text}");
        assert!(text.contains("hit by"), "{text}");
        assert!(report.target.is_some(), "targets turn the guided search on: {report}");
    }

    #[cfg(all(crabcheck_instrumented, feature = "coverage"))]
    #[test]
    fn novelty_counts_what_an_input_reached_first() {
        let mut config = cfg();
        config.seed = Some(1);
        config.coverage.enabled = true;
        let mut run = Run::new(&config, gens::int(0i32..=1), |x: i32| branchy(x) > 0);
        let input = run.draw().expect("ints never discard");
        let first = run.test(&input);
        assert!(first.novel() && first.novelty() > 0, "{first:?}");
        let again = run.test(&input);
        assert!(!again.novel(), "{again:?}");
        assert_eq!(again.novelty(), 0, "the same input reaches nothing new");
        let other = Input { value: 2000, ..input.clone() };
        let differs = run.test(&other);
        assert!(differs.novelty() > 0, "the other branch is new: {differs:?}");
        run.finish().assert_passed();
    }

    #[cfg(all(crabcheck_instrumented, feature = "coverage", feature = "json"))]
    #[test]
    fn the_coverage_grid_is_written() {
        use crate::config::CoverageTarget;
        let dir = std::env::temp_dir().join(format!("crabcheck-grid-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut config = cfg().name("grid::branchy");
        config.tests = 50;
        config.coverage.html = Some(dir.clone());
        config.coverage.targets =
            vec![CoverageTarget::new(file!(), BRANCHY_LINE + 2..=BRANCHY_LINE + 2)];
        config.coverage.targets_file = None;
        let report = check(&config, gens::int(-100i32..=100), |x| branchy(x) == 2);
        let page = report.coverage_html.clone().expect("grid written");
        assert_eq!(page, dir.join("grid_branchy.html"));
        assert!(report.to_string().contains("coverage grid written to"), "{report}");
        let html = std::fs::read_to_string(&page).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(html.contains("crabcheck::runner::tests::branchy"), "function listed");
        assert!(html.contains("\"file_index\""), "targets embedded");
        assert!(html.contains("fn branchy(x: i32) -> i32 {"), "source embedded");
        // The page embeds this very file as source, so look at the script
        // tag rather than for the placeholder's absence.
        assert!(html.contains("<script id=\"data\" type=\"application/json\">{"), "data embedded");
        let start = html.find("<script id=\"data\"").unwrap();
        let end = html[start..].find("</script>").unwrap() + start;
        let json_text = &html[html[start..].find('>').unwrap() + start + 1..end];
        let data: serde_json::Value =
            serde_json::from_str(json_text).expect("embedded JSON parses");
        assert_eq!(data["property"], "grid::branchy");
        let property_fn = data["property_fn"].as_str().unwrap_or_default();
        assert!(
            property_fn
                .starts_with("crabcheck::runner::tests::the_coverage_grid_is_written::{closure#"),
            "{property_fn}"
        );
        assert!(
            data["files"].as_array().unwrap().iter().any(|f| {
                f["functions"].as_array().unwrap().iter().any(|func| {
                    func["name"] == "crabcheck::runner::tests::branchy"
                        && func["lines"].as_array().unwrap().iter().any(|l| l[1] == 0)
                        && func["lines"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|l| l[1].as_u64().unwrap() > 0)
                })
            }),
            "{data}"
        );
        assert_eq!(data["targets"][0]["hits"], 0);
    }

    #[cfg(feature = "coverage")]
    #[test]
    fn coverage_summary_matches_the_build() {
        let report = check(&cfg(), gens::vec(gens::int(0..=9)), |v: Vec<i32>| v.len() <= 100);
        if crate::coverage::available() {
            let coverage = report.coverage.as_ref().expect("coverage collected");
            assert!(coverage.functions_total > 0);
            assert_eq!(coverage.inputs, 64);
            assert!(report.to_string().contains("coverage:"), "{report}");
        } else {
            assert!(report.coverage.is_none());
            assert!(
                !report.health.iter().any(|w| w.kind.starts_with("coverage")),
                "{:?}",
                report.health
            );
        }
        let mut wants = cfg();
        wants.coverage.guided = true;
        let report = check(&wants, gens::vec(gens::int(0..=9)), |v: Vec<i32>| v.len() <= 100);
        if crate::coverage::available() {
            assert!(report.target.is_some(), "guided search seeds the pool from novelty: {report}");
        } else {
            assert!(
                report.health.iter().any(|w| w.kind == "coverage_unavailable"),
                "{:?}",
                report.health
            );
        }
    }

    #[test]
    fn hand_written_loops_replay_by_seed() {
        fn climb(seed: u64) -> Report {
            let mut config = cfg();
            config.seed = Some(seed);
            config.tests = 100;
            config.max_size = 20;
            let mut run = Run::new(&config, gens::vec(gens::int(0i32..=9)), |v: Vec<i32>| {
                let sum: i32 = v.iter().sum();
                crate::target(f64::from(sum));
                sum < 500
            });
            while run.wants_more() {
                let Some(input) = run.draw() else { continue };
                let tested = run.test(&input);
                if tested.passed() {
                    run.keep(input.value, &tested);
                }
            }
            for _ in 0..200 {
                if run.report().status != Status::Passed {
                    break;
                }
                let Some(parent) = run.pool_pick() else { break };
                let Some(child) = run.mutate(&parent) else { continue };
                let tested = run.test(&child);
                if tested.failed() {
                    break;
                }
                run.keep(child.value, &tested);
            }
            run.finish()
        }
        let a = climb(11);
        let b = climb(11);
        assert_eq!(a.status, Status::Passed, "{a}");
        assert_eq!((a.tests, a.iterations), (b.tests, b.iterations));
        assert_eq!(a.iterations, 300, "100 draws and 200 mutations: {a}");
        assert_eq!(a.tests + a.discarded, a.iterations, "passing mutants count as tests");
        let target = a.target.as_ref().expect("mutations are summarised");
        assert_eq!(target.iterations, 200);
        assert_eq!(target.best_score, b.target.as_ref().unwrap().best_score);
        assert_ne!(climb(12).seed, a.seed);
    }

    #[test]
    fn a_failure_found_by_mutation_is_reported_without_a_replay_point() {
        let mut config = cfg();
        config.seed = Some(5);
        config.max_size = 8;
        let mut run = Run::new(&config, gens::vec(gens::int(0i32..=9)), |v: Vec<i32>| v.is_empty());
        let input = loop {
            if let Some(input) = run.draw()
                && !input.value.is_empty()
            {
                break input;
            }
        };
        assert!(input.replay().is_some(), "drawn inputs regenerate from a point");
        let mutant = loop {
            if let Some(mutant) = run.mutate(&input.value) {
                break mutant;
            }
        };
        assert!(mutant.replay().is_none(), "mutants do not");
        assert_eq!(mutant.origin(), crate::report::Origin::Mutation);
        let tested = run.test(&mutant);
        assert!(tested.failed());
        assert!(!run.wants_more(), "a failure stops the loop");
        let report = run.finish();
        assert_eq!(report.status, Status::Failed, "{report}");
        let failure = report.failure.expect("failure");
        assert!(failure.replay.is_none(), "found by mutation: {failure:?}");
        assert_eq!(failure.original.iteration, mutant.iteration());
        assert_eq!(report.tests, 0, "nothing passed");
    }

    #[test]
    fn loops_can_shrink_on_their_own_terms() {
        fn failing_run() -> Run<impl Gen<Value = Vec<i32>>, impl FnMut(Vec<i32>) -> bool> {
            let mut config = cfg();
            config.seed = Some(3);
            config.max_size = 30;
            let mut run = Run::new(&config, gens::vec(gens::int(0i32..=9)), |v: Vec<i32>| {
                v.iter().sum::<i32>() < 20
            });
            while run.wants_more() {
                let Some(input) = run.draw() else { continue };
                run.test(&input);
            }
            assert!(run.failure().is_some(), "{}", run.report());
            run
        }

        // The built-in driver, on demand.
        let mut run = failing_run();
        let original = run.failure().unwrap().clone();
        assert!(run.reproduces(&original), "the original reproduces");
        assert!(!run.reproduces(&vec![]), "an empty list passes");
        let minimal = run.shrink().unwrap().clone();
        let sum: i32 = minimal.iter().sum();
        assert!(
            sum >= 20 && sum <= original.iter().sum() && minimal.len() <= original.len(),
            "{minimal:?}"
        );
        let report = run.finish();
        let failure = report.failure.expect("failed");
        assert_eq!(failure.shrunk.as_ref().map(|c| c.debug.clone()), Some(format!("{minimal:?}")));
        assert!(failure.shrink.unwrap().steps >= 3, "the loop's steps count too");

        // A greedy shrinker written by hand: drop one element at a time.
        let mut run = failing_run();
        let mut current = run.failure().unwrap().clone();
        'outer: loop {
            for i in 0..current.len() {
                let mut candidate = current.clone();
                candidate.remove(i);
                if run.reproduces(&candidate) {
                    current = candidate;
                    continue 'outer;
                }
            }
            break;
        }
        assert!(run.shrunk_to(current.clone()));
        let report = run.finish();
        let failure = report.failure.as_ref().expect("failed");
        assert_eq!(failure.shrunk.as_ref().map(|c| c.debug.clone()), Some(format!("{current:?}")));
        assert!(
            current.iter().sum::<i32>() >= 20 && current.iter().sum::<i32>() < 30,
            "{current:?}"
        );
        assert!(!report.health.iter().any(|w| w.kind == "shrunk_to"), "{report}");

        // A wrong claim is refused.
        let mut run = failing_run();
        assert!(!run.shrunk_to(vec![1]));
        let report = run.finish();
        assert!(report.health.iter().any(|w| w.kind == "shrunk_to"), "{report}");
        assert!(report.failure.unwrap().shrunk.is_none(), "the original stands");
    }

    #[test]
    fn size_distribution_skews_small_and_reaches_the_maximum() {
        let sizes: Vec<usize> = (0..4000).map(|i| size_for(9, i, 100)).collect();
        let mean = sizes.iter().sum::<usize>() as f64 / sizes.len() as f64;
        assert!((25.0..45.0).contains(&mean), "mean {mean}");
        let at_max = sizes.iter().filter(|&&s| s == 100).count();
        assert!(at_max > 150, "max reached {at_max} times");
        assert!(sizes.iter().filter(|&&s| s < 10).count() > 600);
        assert_eq!(size_for(9, 17, 100), size_for(9, 17, 100), "pure in (seed, iteration)");
        assert_eq!(size_for(9, 17, 0), 0);
    }

    #[cfg(feature = "json")]
    #[test]
    fn report_serialises_to_json() {
        let report = check(&cfg(), gens::int(0..=9), |x: i32| x < 5);
        let json = report.to_json();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["status"], "Failed");
        assert_eq!(value["seed"], 1);
        assert_eq!(value["failure"]["shrunk"]["debug"], "5");
        assert_eq!(value["failure"]["replay"]["seed"], 1);
        assert!(value["failure"]["shrink"]["steps"].is_number());
    }
}
