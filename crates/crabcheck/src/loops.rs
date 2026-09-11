//! Testing loops shipped with the library, written on the steps of [`Run`]
//! the way Etna's PropLang ships its loops: a *seed pool* policy decides
//! whether the next input is generated afresh or mutated from a seed, and
//! the *feedback* from each test decides whether the input joins the pool.
//!
//! Three loops share that shape and differ in their feedback:
//!
//! - [`fuzz`]: coverage. The feedback is how many counters, or hit-count
//!   classes, an input reached for the first time
//!   ([`Tested::novelty`](crate::Tested::novelty));
//!   inputs that widen coverage become seeds. Needs an instrumented build
//!   with coverage on; without it every feedback is zero and the loop is
//!   random testing.
//! - [`target`]: the score the property reports through
//!   [`target`](crate::target).
//! - [`perf_fuzz`]: how long the property took, for finding slow inputs.
//!
//! After a failure, `localize` (feature `coverage`) is a loop of a
//! different kind: it keeps
//! testing inputs around the minimal failing one until the ranking of
//! suspect functions stops changing.
//!
//! The pools are the policies from PropLang: [`Fifo`] and [`Filo`]
//! queues whose seeds carry an energy spent by useless mutations, a
//! [`Heap`] that always mutates the best seed, three [`Singleton`]
//! policies, and a [`DoubleQueue`] that also keeps inputs the property
//! discarded, at a lower priority, for properties whose preconditions
//! are hard to satisfy. A pool is yours to write too: implement
//! [`SeedPool`].
//!
//! ```
//! use crabcheck::{Config, Run, gens, loops::{self, SeedPool}, target};
//!
//! let config = Config::default().seed(1).tests(200);
//! let mut run = Run::new(&config, gens::vec(gens::int(0i32..=99)), |v: Vec<i32>| {
//!     target(v.iter().filter(|&&x| x > 90).count() as f64);
//!     v.len() <= 100
//! });
//! let mut pool = loops::Heap::new();
//! loops::target(&mut run, &mut pool, 400);
//! let best = pool.best().map(|seed| seed.feedback.score);
//! run.finish().assert_passed();
//! assert!(best.is_some());
//! ```
//!
//! Every loop runs for `fuel` iterations (each a draw or a mutation) or
//! until the property fails or the generator panics, and leaves the
//! report to [`Run::finish`]. The steps behind them are public, so a loop
//! that fits none of the three is a page of code: see
//! `examples/custom_loop.rs`.

use std::fmt::Debug;

use crate::{
    Gen,
    Run,
    Status,
    Testable,
};

/// What a pool asks the loop to do next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Directive<T> {
    /// Draw a fresh input from the generator.
    Generate,
    /// Mutate this seed.
    Mutate(T),
}

/// An input worth mutating, with the feedback that earned it a place and
/// the mutations it may still be spent on.
#[derive(Clone, Debug, PartialEq)]
pub struct Seed<T, F> {
    /// The input.
    pub input: T,
    /// The feedback it produced.
    pub feedback: F,
    /// Useless mutations left before the pool lets it go.
    pub energy: u32,
}

/// The feedback the shipped loops give their pools: whether the property
/// accepted the input, and a score (novelty, the property's target, or
/// the time it took). Ordered by score alone, so pools rank valid and
/// invalid seeds on the same scale.
#[derive(Clone, Copy, Debug)]
pub struct Validity {
    /// The property ran to a verdict rather than discarding the input.
    pub valid: bool,
    /// The feedback's value; higher is more interesting.
    pub score: f64,
}

impl PartialEq for Validity {
    fn eq(&self, other: &Self) -> bool {
        self.score == other.score
    }
}

impl PartialOrd for Validity {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.score.partial_cmp(&other.score)
    }
}

/// A seed pool: the policy behind a loop's choice between generating and
/// mutating, and what it keeps.
pub trait SeedPool<T, F> {
    /// Keep `input` as a seed; `feedback` is what earned it.
    fn invest(&mut self, input: T, feedback: F);

    /// A mutation of the sampled seed was not useful: spend its energy.
    fn revise(&mut self);

    /// What to do next: generate afresh, or mutate a copy of a seed.
    fn sample(&self) -> Directive<T>;

    /// The seed with the best feedback.
    fn best(&self) -> Option<&Seed<T, F>>;

    /// Whether `feedback` is worth investing. Hill climbing by default:
    /// better than the best seed's, or any feedback while the pool is
    /// empty.
    fn useful(&self, feedback: &F) -> bool
    where
        F: PartialOrd,
    {
        self.best().is_none_or(|best| *feedback > best.feedback)
    }

    /// Whether inputs the property discarded may be invested (with
    /// `valid` false in their feedback). Off unless a pool has a place for
    /// them, like [`DoubleQueue`].
    fn wants_invalid(&self) -> bool {
        false
    }
}

fn best_of<'a, T: 'a, F: PartialOrd + 'a>(
    seeds: impl Iterator<Item = &'a Seed<T, F>>,
) -> Option<&'a Seed<T, F>> {
    seeds.fold(None, |best: Option<&Seed<T, F>>, seed| {
        match best {
            Some(best) if seed.feedback > best.feedback => Some(seed),
            Some(best) => Some(best),
            None => Some(seed),
        }
    })
}

/// A first-in, first-out queue of seeds, each with [`Fifo::ENERGY`]
/// mutations. The next seed to mutate is the oldest; a useless mutation
/// costs it one energy and sends it to the back, and a seed with none
/// left is dropped.
#[derive(Clone, Debug)]
pub struct Fifo<T, F> {
    seeds: std::collections::VecDeque<Seed<T, F>>,
    energy: u32,
}

impl<T, F> Fifo<T, F> {
    /// Mutations a new seed gets.
    pub const ENERGY: u32 = 100;

    /// An empty queue.
    pub fn new() -> Self {
        Self::with_energy(Self::ENERGY)
    }

    /// An empty queue whose seeds get `energy` mutations each.
    pub fn with_energy(energy: u32) -> Self {
        Fifo { seeds: std::collections::VecDeque::new(), energy: energy.max(1) }
    }

    /// The seeds, oldest first.
    pub fn seeds(&self) -> impl Iterator<Item = &Seed<T, F>> {
        self.seeds.iter()
    }
}

impl<T, F> Default for Fifo<T, F> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone, F: Copy + PartialOrd> SeedPool<T, F> for Fifo<T, F> {
    fn invest(&mut self, input: T, feedback: F) {
        self.seeds.push_back(Seed { input, feedback, energy: self.energy });
    }

    fn revise(&mut self) {
        if let Some(mut seed) = self.seeds.pop_front()
            && seed.energy > 0
        {
            seed.energy -= 1;
            self.seeds.push_back(seed);
        }
    }

    fn sample(&self) -> Directive<T> {
        match self.seeds.front() {
            Some(seed) if seed.energy > 0 => Directive::Mutate(seed.input.clone()),
            _ => Directive::Generate,
        }
    }

    fn best(&self) -> Option<&Seed<T, F>> {
        best_of(self.seeds.iter())
    }
}

/// A last-in, first-out stack of seeds, each with a single mutation: the
/// newest seed is mutated once, then dropped.
#[derive(Clone, Debug)]
pub struct Filo<T, F> {
    seeds: Vec<Seed<T, F>>,
    energy: u32,
}

impl<T, F> Filo<T, F> {
    /// Mutations a new seed gets.
    pub const ENERGY: u32 = 1;

    /// An empty stack.
    pub fn new() -> Self {
        Self::with_energy(Self::ENERGY)
    }

    /// An empty stack whose seeds get `energy` mutations each.
    pub fn with_energy(energy: u32) -> Self {
        Filo { seeds: Vec::new(), energy: energy.max(1) }
    }

    /// The seeds, newest first.
    pub fn seeds(&self) -> impl Iterator<Item = &Seed<T, F>> {
        self.seeds.iter().rev()
    }
}

impl<T, F> Default for Filo<T, F> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone, F: Copy + PartialOrd> SeedPool<T, F> for Filo<T, F> {
    fn invest(&mut self, input: T, feedback: F) {
        self.seeds.push(Seed { input, feedback, energy: self.energy });
    }

    fn revise(&mut self) {
        if let Some(mut seed) = self.seeds.pop()
            && seed.energy > 0
        {
            seed.energy -= 1;
            self.seeds.push(seed);
        }
    }

    fn sample(&self) -> Directive<T> {
        match self.seeds.last() {
            Some(seed) if seed.energy > 0 => Directive::Mutate(seed.input.clone()),
            _ => Directive::Generate,
        }
    }

    fn best(&self) -> Option<&Seed<T, F>> {
        best_of(self.seeds.iter())
    }
}

/// Seeds ordered by feedback: the best seed is mutated until its
/// [`Heap::ENERGY`] mutations are spent, then the next best.
#[derive(Clone, Debug)]
pub struct Heap<T, F> {
    seeds: Vec<Seed<T, F>>,
    energy: u32,
}

impl<T, F> Heap<T, F> {
    /// Mutations a new seed gets.
    pub const ENERGY: u32 = 100;

    /// An empty heap.
    pub fn new() -> Self {
        Self::with_energy(Self::ENERGY)
    }

    /// An empty heap whose seeds get `energy` mutations each.
    pub fn with_energy(energy: u32) -> Self {
        Heap { seeds: Vec::new(), energy: energy.max(1) }
    }

    /// The seeds, in no particular order.
    pub fn seeds(&self) -> impl Iterator<Item = &Seed<T, F>> {
        self.seeds.iter()
    }
}

impl<T, F> Default for Heap<T, F> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, F: PartialOrd> Heap<T, F> {
    fn top(&self) -> Option<usize> {
        let mut top: Option<usize> = None;
        for (i, seed) in self.seeds.iter().enumerate() {
            if top.is_none_or(|t| seed.feedback > self.seeds[t].feedback) {
                top = Some(i);
            }
        }
        top
    }
}

impl<T: Clone, F: Copy + PartialOrd> SeedPool<T, F> for Heap<T, F> {
    fn invest(&mut self, input: T, feedback: F) {
        self.seeds.push(Seed { input, feedback, energy: self.energy });
    }

    fn revise(&mut self) {
        if let Some(top) = self.top() {
            if self.seeds[top].energy == 0 {
                self.seeds.swap_remove(top);
            } else {
                self.seeds[top].energy -= 1;
            }
        }
    }

    fn sample(&self) -> Directive<T> {
        match self.top() {
            Some(top) => Directive::Mutate(self.seeds[top].input.clone()),
            None => Directive::Generate,
        }
    }

    fn best(&self) -> Option<&Seed<T, F>> {
        self.top().map(|top| &self.seeds[top])
    }
}

/// How a [`Singleton`] treats its one seed's energy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SingletonPolicy {
    /// The seed is mutated for as long as nothing better replaces it.
    Static,
    /// Every useless mutation spends the seed's energy; once spent, the
    /// loop generates until a better input replaces it.
    Monotonic,
    /// Like `Monotonic`, but a spent seed no longer counts as the best, so
    /// the next passing input replaces it whatever its feedback.
    Resetting,
}

/// One seed at a time: the latest invested input.
#[derive(Clone, Debug)]
pub struct Singleton<T, F> {
    policy: SingletonPolicy,
    seed: Option<Seed<T, F>>,
}

impl<T, F> Singleton<T, F> {
    /// Mutations a new seed gets under the spending policies.
    pub const ENERGY: u32 = 1;

    /// An empty singleton with `policy`.
    pub fn new(policy: SingletonPolicy) -> Self {
        Singleton { policy, seed: None }
    }

    /// The seed, if any.
    pub fn seed(&self) -> Option<&Seed<T, F>> {
        self.seed.as_ref()
    }
}

impl<T: Clone, F: Copy + PartialOrd> SeedPool<T, F> for Singleton<T, F> {
    fn invest(&mut self, input: T, feedback: F) {
        self.seed = Some(Seed { input, feedback, energy: Self::ENERGY });
    }

    fn revise(&mut self) {
        if self.policy != SingletonPolicy::Static
            && let Some(seed) = self.seed.as_mut()
        {
            seed.energy = seed.energy.saturating_sub(1);
        }
    }

    fn sample(&self) -> Directive<T> {
        match &self.seed {
            Some(seed) if self.policy == SingletonPolicy::Static || seed.energy > 0 => {
                Directive::Mutate(seed.input.clone())
            },
            _ => Directive::Generate,
        }
    }

    fn best(&self) -> Option<&Seed<T, F>> {
        match &self.seed {
            Some(seed) if self.policy == SingletonPolicy::Resetting && seed.energy == 0 => None,
            seed => seed.as_ref(),
        }
    }
}

/// Two queues: inputs the property accepted, with
/// [`DoubleQueue::VALID_ENERGY`] mutations each, and inputs it discarded,
/// with [`DoubleQueue::INVALID_ENERGY`], mutated only when no valid seed
/// has energy left. For properties whose preconditions are hard to
/// satisfy: an invalid input close to the precondition is a better seed
/// than a fresh draw. A useless mutation costs every seed one energy.
#[derive(Clone, Debug)]
pub struct DoubleQueue<T> {
    valid: Vec<Seed<T, Validity>>,
    invalid: Vec<Seed<T, Validity>>,
    energies: (u32, u32),
}

impl<T> DoubleQueue<T> {
    /// Mutations a valid seed gets.
    pub const VALID_ENERGY: u32 = 100;
    /// Mutations an invalid seed gets.
    pub const INVALID_ENERGY: u32 = 25;

    /// Two empty queues.
    pub fn new() -> Self {
        Self::with_energies(Self::VALID_ENERGY, Self::INVALID_ENERGY)
    }

    /// Two empty queues whose seeds get `valid` and `invalid` mutations.
    pub fn with_energies(valid: u32, invalid: u32) -> Self {
        DoubleQueue {
            valid: Vec::new(),
            invalid: Vec::new(),
            energies: (valid.max(1), invalid.max(1)),
        }
    }

    /// The valid seeds, then the invalid ones.
    pub fn seeds(&self) -> impl Iterator<Item = &Seed<T, Validity>> {
        self.valid.iter().chain(self.invalid.iter())
    }

    fn usable_best(seeds: &[Seed<T, Validity>]) -> Option<&Seed<T, Validity>> {
        best_of(seeds.iter().filter(|seed| seed.energy > 0))
    }
}

impl<T> Default for DoubleQueue<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone> SeedPool<T, Validity> for DoubleQueue<T> {
    fn invest(&mut self, input: T, feedback: Validity) {
        if feedback.valid {
            self.valid.push(Seed { input, feedback, energy: self.energies.0 });
        } else {
            self.invalid.push(Seed { input, feedback, energy: self.energies.1 });
        }
    }

    fn revise(&mut self) {
        for queue in [&mut self.valid, &mut self.invalid] {
            queue.retain_mut(|seed| {
                seed.energy = seed.energy.saturating_sub(1);
                seed.energy > 0
            });
        }
    }

    fn sample(&self) -> Directive<T> {
        match self.best() {
            Some(seed) => Directive::Mutate(seed.input.clone()),
            None => Directive::Generate,
        }
    }

    fn best(&self) -> Option<&Seed<T, Validity>> {
        Self::usable_best(&self.valid).or_else(|| Self::usable_best(&self.invalid))
    }

    fn wants_invalid(&self) -> bool {
        true
    }
}

/// One iteration of the shared loop shape: ask the pool, draw or mutate,
/// test, turn the verdict into feedback with `score`, and invest it when
/// `useful` says so. Returns whether the loop should go on.
fn step<G, R, P, S>(
    run: &mut Run<G, P>,
    pool: &mut S,
    score: impl FnOnce(&crate::Tested) -> Option<f64>,
    useful: impl FnOnce(&S, &Validity) -> bool,
) -> bool
where
    G: Gen,
    G::Value: Clone + Debug,
    R: Testable,
    P: FnMut(G::Value) -> R,
    S: SeedPool<G::Value, Validity> + ?Sized,
{
    if run.failure().is_some() || run.report().status != Status::Passed {
        return false;
    }
    let (input, mutated) = match pool.sample() {
        Directive::Generate => (run.draw(), false),
        Directive::Mutate(seed) => (run.mutate(&seed), true),
    };
    let Some(input) = input else {
        if mutated {
            pool.revise();
        }
        return true;
    };
    let tested = run.test(&input);
    if tested.failed() {
        return false;
    }
    let feedback = score(&tested).map(|score| Validity { valid: tested.passed(), score });
    let invest = feedback
        .filter(|feedback| feedback.valid || pool.wants_invalid())
        .filter(|feedback| useful(pool, feedback));
    match invest {
        Some(feedback) => pool.invest(input.value, feedback),
        None if mutated => pool.revise(),
        None => {},
    }
    true
}

/// Coverage-guided testing: an input's feedback is its
/// [novelty](crate::Tested::novelty), and every input that reached
/// anything new becomes a seed, whatever the pool's own notion of useful,
/// since in a fuzzer new coverage is always worth keeping. On an
/// uninstrumented build every feedback is zero and nothing is kept.
pub fn fuzz<G, R, P, S>(run: &mut Run<G, P>, pool: &mut S, fuel: u64)
where
    G: Gen,
    G::Value: Clone + Debug,
    R: Testable,
    P: FnMut(G::Value) -> R,
    S: SeedPool<G::Value, Validity> + ?Sized,
{
    for _ in 0..fuel {
        if !step(
            run,
            pool,
            |tested| Some(f64::from(tested.novelty())),
            |_, feedback| feedback.score > 0.0,
        ) {
            break;
        }
    }
}

/// Targeted testing: the feedback is the score the property reports
/// through [`target`](crate::target); inputs the property gave no score
/// are never seeds.
pub fn target<G, R, P, S>(run: &mut Run<G, P>, pool: &mut S, fuel: u64)
where
    G: Gen,
    G::Value: Clone + Debug,
    R: Testable,
    P: FnMut(G::Value) -> R,
    S: SeedPool<G::Value, Validity> + ?Sized,
{
    for _ in 0..fuel {
        if !step(run, pool, |tested| tested.score(), SeedPool::useful) {
            break;
        }
    }
}

/// Performance fuzzing: the feedback is how long the property took, in
/// microseconds, so the pool climbs toward the slowest inputs. The best
/// seed afterwards is the slowest input found.
pub fn perf_fuzz<G, R, P, S>(run: &mut Run<G, P>, pool: &mut S, fuel: u64)
where
    G: Gen,
    G::Value: Clone + Debug,
    R: Testable,
    P: FnMut(G::Value) -> R,
    S: SeedPool<G::Value, Validity> + ?Sized,
{
    for _ in 0..fuel {
        if !step(run, pool, |tested| Some(tested.elapsed().as_secs_f64() * 1e6), SeedPool::useful) {
            break;
        }
    }
}

/// Fault localization as a loop, after a failure: shrink it, then keep
/// mutating the minimal failing input and testing the mutants, whose
/// passing and failing runs sharpen the spectrum the ranking is read
/// from, until the top suspect has not changed for `patience` rounds or
/// `fuel` rounds are spent. Returns the ranking, or `None` without a
/// failure or without coverage. [`Run::finish`] reports whatever the
/// spectrum holds by then.
#[cfg(feature = "coverage")]
pub fn localize<G, R, P>(
    run: &mut Run<G, P>,
    fuel: u64,
    patience: u64,
) -> Option<crate::coverage::Locate>
where
    G: Gen,
    G::Value: Clone + Debug,
    R: Testable,
    P: FnMut(G::Value) -> R,
{
    let minimal = run.shrink()?.clone();
    let mut top: Option<String> = None;
    let mut stable = 0;
    for _ in 0..fuel {
        let Some(input) = run.mutate(&minimal) else {
            if run.report().status != Status::Passed {
                break;
            }
            continue;
        };
        run.test(&input);
        let ranked = run.suspects()?;
        let now = ranked.suspects.first().map(|s| s.function.clone());
        if now == top {
            stable += 1;
            if stable >= patience {
                break;
            }
        } else {
            top = now;
            stable = 0;
        }
    }
    run.suspects()
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{
            Config,
            gens,
        },
    };

    fn scores(n: u32) -> impl Iterator<Item = Validity> {
        (0..n).map(|i| Validity { valid: true, score: f64::from(i) })
    }

    #[test]
    fn fifo_rotates_and_spends_energy() {
        let mut pool: Fifo<u32, Validity> = Fifo::new();
        assert_eq!(pool.sample(), Directive::Generate);
        for (i, feedback) in scores(2).enumerate() {
            assert!(pool.useful(&feedback));
            pool.invest(i as u32, feedback);
        }
        assert!(!pool.useful(&Validity { valid: true, score: 1.0 }), "hill climbing");
        assert_eq!(pool.sample(), Directive::Mutate(0), "the oldest first");
        pool.revise();
        assert_eq!(pool.sample(), Directive::Mutate(1), "a useless mutation rotates the queue");
        assert_eq!(pool.seeds().map(|s| s.energy).collect::<Vec<_>>(), [100, 99]);
        assert_eq!(pool.best().map(|s| s.input), Some(1));
        for _ in 0..300 {
            pool.revise();
        }
        assert_eq!(pool.sample(), Directive::Generate, "spent seeds are dropped");
    }

    #[test]
    fn filo_mutates_the_newest_once() {
        let mut pool: Filo<u32, Validity> = Filo::new();
        for (i, feedback) in scores(2).enumerate() {
            pool.invest(i as u32, feedback);
        }
        assert_eq!(pool.sample(), Directive::Mutate(1));
        pool.revise();
        assert_eq!(pool.sample(), Directive::Generate, "one mutation each");
        pool.revise();
        assert_eq!(pool.sample(), Directive::Mutate(0), "then the one below");
    }

    #[test]
    fn heap_and_singletons_follow_their_policies() {
        let mut heap: Heap<u32, Validity> = Heap::new();
        heap.invest(7, Validity { valid: true, score: 7.0 });
        heap.invest(9, Validity { valid: true, score: 9.0 });
        heap.invest(8, Validity { valid: true, score: 8.0 });
        assert_eq!(heap.sample(), Directive::Mutate(9));
        for _ in 0..=Heap::<u32, Validity>::ENERGY {
            heap.revise();
        }
        assert_eq!(heap.sample(), Directive::Mutate(8), "the best is spent, then the next");

        let mut fixed = Singleton::new(SingletonPolicy::Static);
        fixed.invest(1u32, Validity { valid: true, score: 1.0 });
        fixed.revise();
        assert_eq!(fixed.sample(), Directive::Mutate(1), "static seeds never tire");

        let mut monotonic = Singleton::new(SingletonPolicy::Monotonic);
        monotonic.invest(1u32, Validity { valid: true, score: 1.0 });
        monotonic.revise();
        assert_eq!(monotonic.sample(), Directive::Generate);
        assert!(
            !monotonic.useful(&Validity { valid: true, score: 0.5 }),
            "the seed still sets the bar"
        );

        let mut resetting = Singleton::new(SingletonPolicy::Resetting);
        resetting.invest(1u32, Validity { valid: true, score: 1.0 });
        resetting.revise();
        assert!(
            resetting.useful(&Validity { valid: true, score: 0.5 }),
            "a spent seed no longer does"
        );
    }

    #[test]
    fn the_double_queue_keeps_invalid_inputs_behind_valid_ones() {
        let mut pool: DoubleQueue<u32> = DoubleQueue::new();
        assert!(pool.wants_invalid());
        pool.invest(1, Validity { valid: false, score: 5.0 });
        assert_eq!(pool.sample(), Directive::Mutate(1), "an invalid seed beats a fresh draw");
        pool.invest(2, Validity { valid: true, score: 1.0 });
        assert_eq!(pool.sample(), Directive::Mutate(2), "any valid seed comes first");
        for _ in 0..DoubleQueue::<u32>::INVALID_ENERGY {
            pool.revise();
        }
        assert_eq!(pool.seeds().count(), 1, "spent invalid seeds are dropped");
    }

    fn config(seed: u64) -> Config {
        let mut config = Config::default().seed(seed).tests(60).max_size(20);
        config.regressions.save = false;
        config.regressions.replay = false;
        config.coverage.enabled = false;
        config
    }

    #[test]
    fn the_target_loop_climbs_the_score() {
        let mut run = Run::new(&config(4), gens::vec(gens::int(0i32..=9)), |v: Vec<i32>| {
            crate::target(f64::from(v.iter().sum::<i32>()));
            true
        });
        let mut random_best = 0.0f64;
        while run.wants_more() {
            let Some(input) = run.draw() else { continue };
            random_best = random_best.max(run.test(&input).score().unwrap_or(0.0));
        }
        let mut pool = Heap::new();
        target(&mut run, &mut pool, 300);
        let best = pool.best().expect("seeds were kept");
        assert!(
            best.feedback.valid && best.feedback.score > random_best,
            "{best:?} vs {random_best}"
        );
        let report = run.finish();
        assert_eq!(report.status, Status::Passed, "{report}");
        assert_eq!(report.iterations, 60 + 300);
    }

    #[test]
    fn loops_stop_at_a_failure_and_leave_it_to_finish() {
        let mut run = Run::new(&config(9), gens::vec(gens::int(0i32..=9)), |v: Vec<i32>| {
            crate::target(f64::from(v.len() as u32));
            v.len() < 12
        });
        let mut pool = Fifo::new();
        target(&mut run, &mut pool, 2_000);
        assert!(run.failure().is_some(), "{}", run.report());
        assert!(run.report().iterations < 2_000, "stopped early");
        let report = run.finish();
        assert_eq!(report.status, Status::Failed, "{report}");
        let shrunk = report.failure.unwrap().shrunk.expect("shrunk");
        assert_eq!(shrunk.debug.matches(',').count(), 11, "twelve elements: {}", shrunk.debug);
    }

    /// Eight bytes whose sum must stay under 1900: the mean is 1020 and a
    /// random sum reaches 1900 about once in a hundred thousand draws.
    fn heavy_bytes() -> impl Gen<Value = Vec<u8>> {
        gens::vec(gens::int(0u8..=255)).len(8..=8)
    }

    fn heavy(v: Vec<u8>) -> bool {
        let sum: u32 = v.iter().map(|&x| u32::from(x)).sum();
        crate::target(f64::from(sum));
        sum < 1900
    }

    /// Random testing, then the target loop over the pool. Returns the
    /// best sum random testing saw, the best the pool holds afterwards,
    /// and the sum of the failure if one was found.
    fn climb_with<S: SeedPool<Vec<u8>, Validity>>(
        mut pool: S,
        fuel: u64,
    ) -> (f64, f64, Option<u32>) {
        let mut config = config(11);
        config.tests = 300;
        let mut run = Run::new(&config, heavy_bytes(), heavy);
        let mut random_best = 0.0f64;
        while run.wants_more() {
            let Some(input) = run.draw() else { continue };
            random_best = random_best.max(run.test(&input).score().unwrap_or(0.0));
        }
        assert!(run.failure().is_none(), "random testing does not reach 1900: {}", run.report());
        target(&mut run, &mut pool, fuel);
        let best = pool.best().map_or(0.0, |seed| seed.feedback.score);
        let sum = run.failure().map(|v| v.iter().map(|&x| u32::from(x)).sum());
        run.finish();
        (random_best, best, sum)
    }

    #[test]
    fn pools_that_mutate_their_best_seed_climb_to_what_random_testing_misses() {
        for (name, (random_best, best, found)) in [
            ("heap", climb_with(Heap::new(), 5_000)),
            ("static singleton", climb_with(Singleton::new(SingletonPolicy::Static), 5_000)),
            ("double queue", climb_with(DoubleQueue::new(), 5_000)),
        ] {
            assert!(random_best < 1900.0, "{name}: {random_best}");
            let sum =
                found.unwrap_or_else(|| panic!("the {name} pool did not reach 1900 (best {best})"));
            assert!(sum >= 1900, "{name}: {sum}");
        }
    }

    #[test]
    fn a_queue_climbs_slowly_and_a_stack_not_at_all() {
        // A FIFO mutates its oldest seed, whose mutants rarely beat the best
        // in the queue; it still moves far past what random testing saw.
        let (random_best, best, _) = climb_with(Fifo::new(), 5_000);
        assert!(best > random_best + 200.0, "fifo: {best} after random {random_best}");
        // One mutation per seed is a random walk, not a climb.
        let (random_best, best, found) = climb_with(Filo::new(), 2_000);
        assert!(
            found.is_none() && best < random_best + 300.0,
            "filo: {best} after random {random_best}"
        );
    }

    /// Six magic bytes, each behind the previous: reaching step `k`
    /// executes a branch no input with fewer right bytes executes, so
    /// coverage rewards every step, while a random draw finishes the
    /// staircase once in 4096.
    #[cfg(all(crabcheck_instrumented, feature = "coverage"))]
    #[rustfmt::skip]
    fn stairs(v: &[u8]) -> usize {
        if v.len() < 6 || v[0] != 3 { return 0; }
        if v[1] != 1 { return 1; }
        if v[2] != 2 { return 2; }
        if v[3] != 0 { return 3; }
        if v[4] != 3 { return 4; }
        if v[5] != 1 { return 5; }
        6
    }

    #[cfg(all(crabcheck_instrumented, feature = "coverage"))]
    fn staircase() -> impl Gen<Value = Vec<u8>> {
        gens::vec(gens::int(0u8..=3)).len(6..=6)
    }

    #[cfg(all(crabcheck_instrumented, feature = "coverage"))]
    #[test]
    fn fuzz_climbs_a_staircase_by_coverage_alone() {
        let mut config = config(5);
        config.tests = 400;
        config.coverage.enabled = true;
        let mut run = Run::new(&config, staircase(), |v: Vec<u8>| stairs(&v) < 6);
        while run.wants_more() {
            let Some(input) = run.draw() else { continue };
            run.test(&input);
        }
        assert!(
            run.failure().is_none(),
            "random draws do not finish the staircase: {}",
            run.report()
        );
        run.finish();

        // A fresh run: the property reports no score, so only coverage can
        // guide this, and the loop starts from nothing.
        let mut run = Run::new(&config, staircase(), |v: Vec<u8>| stairs(&v) < 6);
        let mut pool = Fifo::with_energy(1_000);
        fuzz(&mut run, &mut pool, 3_000);
        assert_eq!(run.failure(), Some(&vec![3, 1, 2, 0, 3, 1]), "{}", run.report());
        // Every rung reached became a seed; the mutator's swaps can skip
        // rungs on the way up, so not every rung is visited.
        let mut steps: Vec<usize> = pool.seeds().map(|seed| stairs(&seed.input)).collect();
        steps.sort_unstable();
        steps.dedup();
        assert!(
            steps.len() >= 3 && steps.contains(&1) && steps.contains(&2),
            "the climb: {steps:?}"
        );
        let report = run.finish();
        assert_eq!(report.status, Status::Failed);
        assert!(report.coverage.as_ref().is_some_and(|c| c.novel_inputs >= 6), "{report}");
    }

    /// A function with a planted bug on one branch, and an innocent one.
    #[cfg(all(crabcheck_instrumented, feature = "coverage"))]
    #[rustfmt::skip]
    fn culprit(x: i32) -> i32 {
        if x > 900 { x - 1 } else { x + 1 }
    }

    #[cfg(all(crabcheck_instrumented, feature = "coverage"))]
    fn innocent(x: i32) -> i32 {
        x.wrapping_mul(3)
    }

    #[cfg(all(crabcheck_instrumented, feature = "coverage"))]
    #[test]
    fn the_localizing_loop_ranks_the_planted_bug_first_and_settles() {
        let mut config = config(8);
        config.tests = 400;
        config.coverage.enabled = true;
        config.locate.enabled = true;
        let mut run = Run::new(&config, gens::int(0i32..=1000), |x: i32| {
            let _ = innocent(x);
            culprit(x) == x + 1
        });
        while run.wants_more() {
            let Some(input) = run.draw() else { continue };
            run.test(&input);
        }
        assert!(run.failure().is_some(), "{}", run.report());
        let before = run.report().iterations;
        let ranked = localize(&mut run, 300, 25).expect("a ranking");
        let rounds = run.report().iterations - before;
        assert!(rounds < 300, "the ranking settled early: {rounds} rounds");
        let top = &ranked.suspects[0];
        assert!(top.function.ends_with("::culprit"), "{ranked:?}");
        assert!(ranked.suspects.iter().all(|s| !s.function.ends_with("::innocent") || s.rank > 1));
        assert!(ranked.failing >= 2 && ranked.passing > ranked.failing, "{ranked:?}");
        let report = run.finish();
        assert_eq!(report.status, Status::Failed, "{report}");
        let reported = report.locate.as_ref().expect("localized");
        assert!(reported.suspects[0].function.ends_with("::culprit"), "{report}");
    }

    /// Innocent, but with a branch only inputs past 850 take: every
    /// failure (past 900) takes it, and so does one passing input in
    /// twenty.
    #[cfg(all(crabcheck_instrumented, feature = "coverage"))]
    #[rustfmt::skip]
    fn rival(x: i32) -> i32 {
        if x > 850 { x * 3 } else { x * 2 }
    }

    #[cfg(all(crabcheck_instrumented, feature = "coverage"))]
    #[test]
    fn the_localizing_loop_breaks_the_tie_a_first_failure_leaves() {
        use crate::coverage::Locate;
        fn ochiai(ranking: &Locate, name: &str) -> Option<f64> {
            ranking.suspects.iter().find(|s| s.function.ends_with(name)).map(|s| s.scores.ochiai)
        }
        // Test until the first failure. With a seed whose first failure
        // comes before any passing input past 850, the rival's branch has
        // been executed by every failing run and no passing one, exactly
        // like the culprit's: coverage alone cannot tell them apart.
        let (mut run, before) = (1..64)
            .find_map(|seed| {
                let mut config = config(seed);
                config.tests = 400;
                config.coverage.enabled = true;
                config.locate.enabled = true;
                let mut run = Run::new(&config, gens::int(0i32..=1000), |x: i32| {
                    let _ = rival(x);
                    culprit(x) == x + 1
                });
                while run.failure().is_none() && run.wants_more() {
                    if let Some(input) = run.draw() {
                        run.test(&input);
                    }
                }
                let before = run.suspects()?;
                (ochiai(&before, "::rival") == Some(1.0)).then_some((run, before))
            })
            .expect("a seed whose first failure precedes any passing input past 850");
        assert_eq!(before.failing, 1, "{before:?}");
        assert_eq!(ochiai(&before, "::culprit"), Some(1.0), "{before:?}");

        let after = localize(&mut run, 300, 25).expect("a ranking");
        assert!(after.failing > 1 && after.passing > before.passing, "{after:?}");
        assert!(after.suspects[0].function.ends_with("::culprit"), "{after:?}");
        assert_eq!(ochiai(&after, "::culprit"), Some(1.0), "{after:?}");
        let rival_after = ochiai(&after, "::rival").expect("still listed");
        assert!(rival_after < 1.0, "passing mutants near 901 take the rival's branch: {after:?}");
        run.finish();
    }

    #[cfg(all(crabcheck_instrumented, feature = "coverage"))]
    #[test]
    fn the_built_in_guided_search_climbs_the_staircase_too() {
        let mut random_only = config(5);
        random_only.tests = 400;
        random_only.coverage.enabled = true;
        random_only.target.tests = 0;
        let report = crate::check(&random_only, staircase(), |v: Vec<u8>| stairs(&v) < 6);
        assert_eq!(report.status, Status::Passed, "{report}");

        let mut guided = random_only.clone();
        guided.coverage.guided = true;
        guided.target.tests = 3_000;
        guided.target.energy = 1_000;
        let report = crate::check(&guided, staircase(), |v: Vec<u8>| stairs(&v) < 6);
        assert_eq!(report.status, Status::Failed, "{report}");
        let failure = report.failure.as_ref().unwrap();
        assert_eq!(failure.counterexample().debug, "[3, 1, 2, 0, 3, 1]");
        assert!(report.to_string().contains("found in the targeted phase"), "{report}");
    }

    #[test]
    fn fuzz_without_coverage_is_random_testing_and_perf_fuzz_finds_slow_inputs() {
        let mut run =
            Run::new(&config(2), gens::vec(gens::int(0i32..=9)), |v: Vec<i32>| v.len() <= 20);
        let mut pool: Fifo<Vec<i32>, Validity> = Fifo::new();
        fuzz(&mut run, &mut pool, 50);
        assert!(pool.best().is_none_or(|s| s.feedback.score == 0.0), "no coverage, no novelty");
        assert_eq!(run.report().iterations, 50);
        run.finish().assert_passed();

        let mut run = Run::new(&config(3), gens::vec(gens::int(0i32..=9)), |v: Vec<i32>| {
            std::thread::sleep(std::time::Duration::from_micros(20 * v.len() as u64));
            true
        });
        let mut pool = Heap::new();
        perf_fuzz(&mut run, &mut pool, 80);
        let best = pool.best().expect("a slowest input");
        assert!(best.feedback.score >= 20.0 * best.input.len() as f64 * 0.5, "{best:?}");
        run.finish().assert_passed();
    }
}
