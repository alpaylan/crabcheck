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
    let (config, problems) = config.clone().apply_env_vars(std::env::vars());
    let seed = config.seed.unwrap_or_else(random_seed);
    let mut run = Run::new(&config, &generator, property, seed, property_name(&config));
    for problem in problems {
        run.report.warn("config_env", problem);
    }
    let _coverage_guard = run.enable_coverage();

    if let Some(point) = config.replay {
        let attempt = run.run_point(point);
        let reproduced = matches!(attempt, Attempt::Failed { .. });
        run.report.replay = Some(ReplayOutcome { point, reproduced });
        if let Attempt::Failed { size, result } = attempt
            && let Some(value) = run.regenerate(point)
        {
            run.fail(
                Found { value, point: Some(point), size, iteration: point.iteration, result },
                None,
            );
            return run.finish();
        }
    }

    #[cfg(feature = "json")]
    if config.regressions.replay {
        let entries = crate::regressions::load(&config.regressions.dir, &run.name);
        if !entries.is_empty() {
            run.report.regressions.file =
                Some(crate::regressions::path_for(&config.regressions.dir, &run.name));
        }
        for entry in entries {
            let point = ReplayPoint { seed: entry.seed, iteration: entry.iteration };
            run.report.regressions.replayed += 1;
            let Some(value) = run.regenerate(point) else {
                run.report.regressions.stale += 1;
                run.report.warn("regressions", format!("stored input {point} could not be regenerated (discarded or the generator panicked)"));
                continue;
            };
            let (debug, _, _) = render(&value, config.report.truncate_debug_at);
            if debug != entry.debug {
                run.report.regressions.stale += 1;
                run.report.warn(
                    "regressions",
                    format!(
                        "stored input {point} no longer matches the generator (stored {}, regenerated {debug}); \
                         delete the entry or rerun the property",
                        entry.debug
                    ),
                );
                continue;
            }
            let size = size_for(point.seed, point.iteration, config.max_size);
            let (result, _) = run.evaluate(value.clone());
            if Signature::of(&result).is_some() {
                run.fail(
                    Found { value, point: Some(point), size, iteration: point.iteration, result },
                    None,
                );
                return run.finish();
            }
        }
    }

    if let Some(found) = run.random_search(None) {
        run.fail(found, None);
    } else if run.report.status == Status::Passed {
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
    let mut run = Run::new(&config, &generator, &property, seed, property_name(&config));
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

    #[allow(clippy::type_complexity)]
    let outcomes: Vec<(Run<'_, G, &P>, Option<Found<G::Value>>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = worker_configs
            .iter()
            .enumerate()
            .map(|(index, worker_config)| {
                let stop = &stop;
                scope.spawn(move || {
                    let worker_seed = mix(seed, u64::MAX - 2 - index as u64);
                    let mut worker = Run::new(
                        worker_config,
                        generator_ref,
                        property_ref,
                        worker_seed,
                        String::new(),
                    );
                    worker.started = started;
                    let found = worker.random_search(Some(stop));
                    if found.is_some() {
                        stop.store(true, Ordering::Relaxed);
                    }
                    (worker, found)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("worker thread panicked outside the property"))
            .collect()
    });

    let mut failure: Option<(usize, Found<G::Value>)> = None;
    for (index, (worker, found)) in outcomes.into_iter().enumerate() {
        run.absorb(worker);
        if let Some(found) = found
            && failure.is_none()
        {
            failure = Some((index, found));
        }
    }
    if let Some((thread, found)) = failure {
        run.report.status = Status::Passed;
        run.report.abort = None;
        run.fail(found, Some(thread));
    } else if run.report.status == Status::Passed {
        run.targeted_search();
    }
    run.finish()
}

/// Per-run coverage state: the tracker, the fault-localization spectrum and
/// the signature of the failure being localized.
#[cfg(feature = "coverage")]
struct CoverageState {
    tracker: Option<crate::coverage::tracker::Tracker>,
    spectrum: Option<crate::coverage::locate::Spectrum>,
    signature: Option<Signature>,
}

#[cfg(feature = "coverage")]
impl CoverageState {
    fn new() -> Self {
        CoverageState { tracker: None, spectrum: None, signature: None }
    }

    fn begin(&self) -> Option<crate::coverage::Snapshot> {
        self.tracker.as_ref().and_then(|t| t.begin())
    }

    /// Record what an input did; returns whether it reached new coverage.
    /// Passing runs and runs failing like the first failure feed the
    /// spectrum; discards and other failures do not.
    fn end(
        &mut self,
        before: Option<crate::coverage::Snapshot>,
        result: &Result<Outcome, PanicInfo>,
    ) -> bool {
        let (Some(tracker), Some(before)) = (self.tracker.as_mut(), before) else {
            return false;
        };
        let observation = tracker.end(&before);
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
        observation.novel
    }
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

    fn end(&mut self, _before: Option<()>, _result: &Result<Outcome, PanicInfo>) -> bool {
        false
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

struct Run<'a, G: Gen, P> {
    config: &'a Config,
    generator: &'a G,
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

enum Attempt {
    Passed {
        size: usize,
        labels: Vec<String>,
        score: Option<f64>,
        novel: bool,
        covers: Vec<(String, bool)>,
    },
    Discarded(String),
    Failed {
        size: usize,
        result: Result<Outcome, PanicInfo>,
    },
    GenPanicked(PanicInfo),
}

impl<'a, G, R, P> Run<'a, G, P>
where
    G: Gen,
    G::Value: Clone + Debug,
    R: Testable,
    P: FnMut(G::Value) -> R,
{
    fn new(config: &'a Config, generator: &'a G, property: P, seed: u64, name: String) -> Self {
        Run {
            config,
            generator,
            property,
            report: Report::new(seed, config.max_size),
            seed,
            name,
            started: Instant::now(),
            seen: HashSet::new(),
            generated: 0,
            pool: Pool::new(config.target.pool_size, config.target.energy),
            coverage: CoverageState::new(),
            covers: BTreeMap::new(),
        }
    }

    /// Generate the input at `point`. Accumulates generation time.
    fn generate(&mut self, point: ReplayPoint) -> Result<(G::Value, usize), Attempt> {
        let size = size_for(point.seed, point.iteration, self.config.max_size);
        let mut source = Source::new(mix(point.seed, point.iteration), size)
            .with_filter_max_attempts(self.config.filter_max_attempts);
        let started = Instant::now();
        let generated = panic::catch(|| self.generator.generate(&mut source));
        self.report.gen_time += started.elapsed();
        match generated {
            Err(info) => Err(Attempt::GenPanicked(info)),
            Ok(_) if source.discarded().is_some() => {
                Err(Attempt::Discarded(source.discarded().unwrap_or_default().to_string()))
            },
            Ok(value) => Ok((value, size)),
        }
    }

    /// The value at `point`, if it generates without discard or panic.
    fn regenerate(&mut self, point: ReplayPoint) -> Option<G::Value> {
        self.generate(point).ok().map(|(value, _)| value)
    }

    /// Run the property once. Accumulates property time.
    fn evaluate(&mut self, value: G::Value) -> (Result<Outcome, PanicInfo>, labels::Observed) {
        let started = Instant::now();
        let outcome = evaluate(&mut self.property, value);
        self.report.prop_time += started.elapsed();
        outcome
    }

    /// Generate the input at `point` and run the property on it. Counts
    /// nothing; callers decide what the attempt means.
    fn run_point(&mut self, point: ReplayPoint) -> Attempt {
        let (value, size) = match self.generate(point) {
            Ok(generated) => generated,
            Err(attempt) => return attempt,
        };
        self.generated += 1;
        self.seen.insert(debug_hash(&value));
        let before = self.coverage.begin();
        let (result, observed) = self.evaluate(value);
        let novel = self.coverage.end(before, &result);
        match result {
            Ok(Outcome::Pass) => {
                Attempt::Passed {
                    size,
                    labels: observed.labels,
                    score: observed.score,
                    novel,
                    covers: observed.covers,
                }
            },
            Ok(Outcome::Discard(reason)) => Attempt::Discarded(reason),
            result => Attempt::Failed { size, result },
        }
    }

    fn record_discard(&mut self, reason: String) {
        self.report.discarded += 1;
        *self.report.discard_reasons.entry(reason).or_default() += 1;
    }

    fn record_pass(&mut self, size: usize, labels: Vec<String>, covers: Vec<(String, bool)>) {
        self.report.tests += 1;
        *self.report.sizes.entry(size).or_default() += 1;
        for label in labels {
            *self.report.labels.entry(label).or_default() += 1;
        }
        for (label, met) in covers {
            let entry = self.covers.entry(label).or_insert((0, 0));
            entry.0 += u64::from(met);
            entry.1 += 1;
        }
    }

    /// Whether coverage-guided search is on and coverage is being collected.
    fn guided(&self) -> bool {
        #[cfg(feature = "coverage")]
        {
            self.config.coverage.guided && self.coverage.tracker.is_some()
        }
        #[cfg(not(feature = "coverage"))]
        {
            false
        }
    }

    #[cfg(feature = "coverage")]
    /// Start collecting coverage if configured and available. Returns the
    /// process-wide coverage lock, which the caller holds for the run.
    fn enable_coverage(&mut self) -> Option<std::sync::MutexGuard<'static, ()>> {
        let settings = &self.config.coverage;
        if !settings.enabled {
            return None;
        }
        if !crate::coverage::available() {
            if settings.guided || settings.profraw_dir.is_some() {
                self.report.warn(
                    "coverage_unavailable",
                    "coverage was requested but this binary is not instrumented; build with RUSTFLAGS=\"-C instrument-coverage\"",
                );
            }
            return None;
        }
        let filter = if settings.filter.is_empty() {
            crate_of(std::any::type_name::<P>()).to_string()
        } else {
            settings.filter.clone()
        };
        match crate::coverage::tracker::Tracker::new(&filter) {
            Some(tracker) => {
                let guard = crate::coverage::lock();
                if self.config.locate.enabled {
                    self.coverage.spectrum =
                        crate::coverage::locate::Spectrum::new(tracker.selected());
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

    #[cfg(not(feature = "coverage"))]
    fn enable_coverage(&mut self) -> Option<()> {
        None
    }


    /// The random-testing loop. Returns the first failing input, which the
    /// caller hands to [`fail`](Self::fail). `stop` lets parallel workers
    /// end early once another worker has found a failure.
    fn random_search(&mut self, stop: Option<&AtomicBool>) -> Option<Found<G::Value>> {
        let config = self.config;
        let max_discards = (config.max_discard_ratio * config.tests as f64).max(0.0) as u64;
        let budget = config.budget_ms.map(std::time::Duration::from_millis);

        while self.report.tests < config.tests {
            if stop.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                return None;
            }
            if let Some(budget) = budget
                && self.started.elapsed() > budget
            {
                self.report.stopped_by_budget = true;
                return None;
            }
            if let Some(timeout) = config.timeout_duration()
                && self.started.elapsed() > timeout
            {
                self.report.status = Status::TimedOut;
                return None;
            }
            if self.report.discarded > max_discards {
                self.report.status = Status::GaveUp;
                return None;
            }

            let iteration = self.report.iterations;
            self.report.iterations += 1;
            let point = ReplayPoint { seed: self.seed, iteration };
            match self.run_point(point) {
                Attempt::GenPanicked(info) => {
                    self.report.status = Status::Aborted;
                    self.report.abort =
                        Some(Abort { message: info.message, location: info.location, iteration });
                    return None;
                },
                Attempt::Discarded(reason) => self.record_discard(reason),
                Attempt::Passed { size, labels, score, novel, covers } => {
                    self.record_pass(size, labels, covers);
                    self.keep_if_interesting(point, score, novel);
                },
                Attempt::Failed { size, result } => {
                    let value = self.regenerate(point)?;
                    return Some(Found { value, point: Some(point), size, iteration, result });
                },
            }
        }
        None
    }

    /// Keep a passing input as a targeted-phase seed when its score improves
    /// the pool, or when guided search is on and it reached new coverage.
    fn keep_if_interesting(&mut self, point: ReplayPoint, score: Option<f64>, novel: bool) {
        let guided = self.guided();
        let by_score = score.is_some_and(|s| self.pool.improves(s));
        if !(by_score || (novel && guided)) {
            return;
        }
        if let Some(value) = self.regenerate(point) {
            if by_score {
                self.pool.add(value, score.unwrap_or(0.0));
            } else {
                self.pool.add_forced(value, score.unwrap_or(0.0));
            }
        }
    }

    /// Fold a worker's statistics into this run.
    fn absorb(&mut self, mut other: Run<'_, G, P>) {
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
    }

    /// The targeted phase: mutate the best-scoring inputs to push the score
    /// up, looking for a failure along the way.
    fn targeted_search(&mut self) {
        let config = self.config;
        if self.pool.is_empty() {
            return;
        }
        let mut rng = ChaCha8Rng::seed_from_u64(mix(self.seed, u64::MAX - 1));
        let mut iterations = 0u64;
        let max_discards = (config.max_discard_ratio * config.target.tests as f64).max(0.0) as u64;
        let mut discarded = 0u64;

        while iterations < config.target.tests {
            if let Some(timeout) = config.timeout_duration()
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
            let iteration = self.report.iterations;
            self.report.iterations += 1;

            if u01(&mut rng) < config.target.explore {
                let point = ReplayPoint { seed: self.seed, iteration };
                match self.run_point(point) {
                    Attempt::GenPanicked(info) => {
                        self.report.status = Status::Aborted;
                        self.report.abort = Some(Abort {
                            message: info.message,
                            location: info.location,
                            iteration,
                        });
                        break;
                    },
                    Attempt::Discarded(reason) => {
                        discarded += 1;
                        self.record_discard(reason);
                    },
                    Attempt::Passed { size, labels, score, novel, covers } => {
                        self.record_pass(size, labels, covers);
                        self.keep_if_interesting(point, score, novel);
                    },
                    Attempt::Failed { size, result } => {
                        if let Some(value) = self.regenerate(point) {
                            self.fail(
                                Found { value, point: Some(point), size, iteration, result },
                                None,
                            );
                        }
                        break;
                    },
                }
                continue;
            }

            let Some(seed) = self.pool.pick(&mut rng) else { break };
            let size = size_for(self.seed, iteration, config.max_size);
            let mut source = Source::new(mix(self.seed, iteration), size)
                .with_filter_max_attempts(config.filter_max_attempts);
            let mutant = match panic::catch(|| self.generator.mutate(&seed.value, &mut source)) {
                Ok(Some(mutant)) => mutant,
                Ok(None) => continue,
                Err(info) => {
                    self.report.status = Status::Aborted;
                    self.report.abort =
                        Some(Abort { message: info.message, location: info.location, iteration });
                    break;
                },
            };
            self.generated += 1;
            self.seen.insert(debug_hash(&mutant));
            let before = self.coverage.begin();
            let (result, observed) = self.evaluate(mutant.clone());
            let novel = self.coverage.end(before, &result);
            match result {
                Ok(Outcome::Pass) => {
                    self.record_pass(size, observed.labels, observed.covers);
                    let guided = self.guided();
                    match observed.score {
                        Some(score) if self.pool.improves(score) => {
                            self.pool.add(mutant, score);
                        },
                        Some(score) if novel && guided => self.pool.add_forced(mutant, score),
                        None if novel && guided => self.pool.add_forced(mutant, 0.0),
                        _ => {},
                    }
                },
                Ok(Outcome::Discard(reason)) => {
                    discarded += 1;
                    self.record_discard(reason);
                },
                result => {
                    self.fail(Found { value: mutant, point: None, size, iteration, result }, None);
                    break;
                },
            }
        }

        if let Some((best_score, best)) = self.pool.best.clone() {
            self.report.target = Some(TargetSummary {
                iterations,
                improvements: self.pool.improvements,
                best_score,
                best_debug: render(&best, config.report.truncate_debug_at).0,
            });
        }
    }

    /// Handle a failing input: check it reproduces, shrink it, remember it,
    /// and fill in the report.
    fn fail(&mut self, found: Found<G::Value>, thread: Option<usize>) {
        let config = self.config;
        let limit = config.report.truncate_debug_at;
        let signature =
            Signature::of(&found.result).expect("fail() is only called for failing results");
        let (kind, mut message, mut location) = details(found.result);
        let (debug, truncated, _) = render(&found.value, limit);

        let rerun = self.evaluate(found.value.clone()).0;
        let flaky = Signature::of(&rerun).as_ref() != Some(&signature);
        if flaky {
            self.report.warn(
                "flaky",
                "the property did not fail the same way when re-run on the failing input; \
                 it depends on something other than the input, and the input was not shrunk",
            );
        }

        let mut shrunk = None;
        let mut shrink_stats = None;
        let mut minimal = found.value.clone();
        if config.shrink.enabled && !flaky {
            let generator = self.generator;
            let property = &mut self.property;
            let coverage = &mut self.coverage;
            let started = Instant::now();
            let result = shrink::shrink(
                &config.shrink,
                found.value.clone(),
                |value| generator.shrink(value),
                |candidate| {
                    let before = coverage.begin();
                    let outcome = evaluate(property, candidate).0;
                    coverage.end(before, &outcome);
                    (Signature::of(&outcome).as_ref() == Some(&signature)).then(|| {
                        let (_, message, location) = details(outcome);
                        (message, location)
                    })
                },
                |value| render(value, limit),
            );
            self.report.prop_time += started.elapsed();
            minimal = result.value.clone();
            if let Some((shrunk_message, shrunk_location)) = result.failure {
                let (debug, truncated, _) = render(&result.value, limit);
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
            if result.stats.budget_exhausted {
                self.report.warn(
                    "shrink_budget",
                    format!(
                        "shrinking stopped at the budget ({} candidates); raise Config::shrink.max_steps or the \
                         timeout for a smaller counterexample",
                        result.stats.steps
                    ),
                );
            }
            shrink_stats = Some(result.stats);
        }
        #[cfg(feature = "coverage")]
        if self.coverage.spectrum.is_some() && !flaky {
            // Passing and failing neighbours of the minimal input sharpen the
            // spectrum where it matters.
            let neighbourhood = mix(self.seed, u64::MAX - 3);
            for i in 0..config.locate.neighbourhood as u64 {
                let mut source = Source::new(mix(neighbourhood, i), found.size);
                if let Some(mutant) = self.generator.mutate(&minimal, &mut source) {
                    let before = self.coverage.begin();
                    let result = self.evaluate(mutant).0;
                    self.coverage.end(before, &result);
                }
            }
            let panic = location.as_ref().map(|l| (l.file.as_str(), l.line));
            self.report.locate =
                self.coverage.spectrum.as_ref().map(|s| s.rank(panic, config.locate.top));
        }
        #[cfg(not(feature = "coverage"))]
        let _ = minimal;

        #[cfg(feature = "json")]
        if config.regressions.save
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
            match crate::regressions::save(&config.regressions.dir, &entry) {
                Ok((path, appended)) => {
                    self.report.regressions.file = Some(path);
                    self.report.regressions.saved = appended;
                },
                Err(e) => {
                    self.report.warn(
                        "regressions",
                        format!("could not write {}: {e}", config.regressions.dir.display()),
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

    /// Final statistics, health checks and report output.
    fn finish(mut self) -> Report {
        self.report.duration = self.started.elapsed();
        self.report.distinct_inputs_ratio =
            if self.generated == 0 { 1.0 } else { self.seen.len() as f64 / self.generated as f64 };
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
                let summary = tracker.summary();
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
        health::assess(&mut self.report, self.config);
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
