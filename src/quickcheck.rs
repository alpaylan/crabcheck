use std::fmt::Debug;

use rand::{
    Rng,
    rngs::ThreadRng,
};


pub trait Arbitrary<R: Rng> {
    fn generate(r: &mut R, n: usize) -> Self;
}

pub trait Mutate<R: Rng> {
    fn mutate(&self, rng: &mut R, n: usize) -> Self;
}


impl<R: Rng> Arbitrary<R> for i32 {
    fn generate(rng: &mut R, n: usize) -> i32 {
        rng.random_range(-(n as i32)..=(n as i32))
    }
}

// Full-range uniform Arbitrary impls for the remaining primitive integer types
// (matching `proptest::any::<T>()`). These are used by workload adapters that
// want to align with proptest's generator distributions rather than the
// size-scaled `i32`/`usize` impls above.
impl<R: Rng> Arbitrary<R> for u8 {
    fn generate(rng: &mut R, _n: usize) -> u8 { rng.random() }
}
impl<R: Rng> Arbitrary<R> for u16 {
    fn generate(rng: &mut R, _n: usize) -> u16 { rng.random() }
}
impl<R: Rng> Arbitrary<R> for u32 {
    fn generate(rng: &mut R, _n: usize) -> u32 { rng.random() }
}
impl<R: Rng> Arbitrary<R> for u64 {
    fn generate(rng: &mut R, _n: usize) -> u64 { rng.random() }
}
impl<R: Rng> Arbitrary<R> for i8 {
    fn generate(rng: &mut R, _n: usize) -> i8 { rng.random() }
}
impl<R: Rng> Arbitrary<R> for i16 {
    fn generate(rng: &mut R, _n: usize) -> i16 { rng.random() }
}
impl<R: Rng> Arbitrary<R> for i64 {
    fn generate(rng: &mut R, _n: usize) -> i64 { rng.random() }
}
impl<R: Rng> Arbitrary<R> for f32 {
    fn generate(rng: &mut R, _n: usize) -> f32 { rng.random() }
}
impl<R: Rng> Arbitrary<R> for f64 {
    fn generate(rng: &mut R, _n: usize) -> f64 { rng.random() }
}

impl<R: Rng> Mutate<R> for i32 {
    fn mutate(&self, rng: &mut R, _n: usize) -> i32 {
        rng.random_range(self - 10..self + 10)
    }
}

impl<R: Rng> Arbitrary<R> for usize {
    fn generate(rng: &mut R, n: usize) -> usize {
        rng.random_range(0..=n)
    }
}

impl<R: Rng> Mutate<R> for usize {
    fn mutate(&self, rng: &mut R, _n: usize) -> usize {
        rng.random_range(self.saturating_sub(10)..=self.saturating_add(10))
    }
}

impl<R: Rng> Arbitrary<R> for bool {
    fn generate(rng: &mut R, _n: usize) -> bool {
        rng.random_bool(0.5)
    }
}

impl<R: Rng> Mutate<R> for bool {
    fn mutate(&self, rng: &mut R, _n: usize) -> bool {
        if rng.random_bool(0.1) { !*self } else { *self }
    }
}

impl<R: Rng, T: Arbitrary<R>> Arbitrary<R> for Vec<T> {
    fn generate(rng: &mut R, n: usize) -> Vec<T> {
        let mut list = Vec::with_capacity(n);
        for _ in 0..n {
            list.push(T::generate(rng, n));
        }
        list
    }
}

impl<R: Rng, T: Mutate<R> + Clone> Mutate<R> for Vec<T> {
    fn mutate(&self, rng: &mut R, n: usize) -> Vec<T> {
        let mut copy = self.clone();

        // Pick a portion of the list and mutate it
        let a = rng.random_range(0..=self.len());
        let b = rng.random_range(a..=self.len());

        for value in copy[a..b].iter_mut() {
            *value = T::mutate(value, rng, n);
        }

        copy
    }
}


impl<R: Rng, T1: Arbitrary<R>, T2: Arbitrary<R>> Arbitrary<R> for (T1, T2) {
    fn generate(rng: &mut R, n: usize) -> (T1, T2) {
        // todo: make this a splittable Rng
        let r1 = T1::generate(rng, n);
        let r2 = T2::generate(rng, n);
        (r1, r2)
    }
}


impl<R: Rng, T1: Arbitrary<R>, T2: Arbitrary<R>, T3: Arbitrary<R>> Arbitrary<R> for (T1, T2, T3) {
    fn generate(rng: &mut R, n: usize) -> (T1, T2, T3) {
        // todo: make this a splittable Rng
        let r1 = T1::generate(rng, n);
        let r2 = T2::generate(rng, n);
        let r3 = T3::generate(rng, n);
        (r1, r2, r3)
    }
}


impl<R: Rng, T1: Arbitrary<R>, T2: Arbitrary<R>, T3: Arbitrary<R>, T4: Arbitrary<R>> Arbitrary<R>
    for (T1, T2, T3, T4)
{
    fn generate(rng: &mut R, n: usize) -> (T1, T2, T3, T4) {
        // todo: make this a splittable Rng
        let r1 = T1::generate(rng, n);
        let r2 = T2::generate(rng, n);
        let r3 = T3::generate(rng, n);
        let r4 = T4::generate(rng, n);
        (r1, r2, r3, r4)
    }
}


impl<
    R: Rng,
    T1: Arbitrary<R>,
    T2: Arbitrary<R>,
    T3: Arbitrary<R>,
    T4: Arbitrary<R>,
    T5: Arbitrary<R>,
> Arbitrary<R> for (T1, T2, T3, T4, T5)
{
    fn generate(rng: &mut R, n: usize) -> (T1, T2, T3, T4, T5) {
        // todo: make this a splittable Rng
        let r1 = T1::generate(rng, n);
        let r2 = T2::generate(rng, n);
        let r3 = T3::generate(rng, n);
        let r4 = T4::generate(rng, n);
        let r5 = T5::generate(rng, n);
        (r1, r2, r3, r4, r5)
    }
}

// Tuple Mutate impls pick a random NON-EMPTY SUBSET of components to mutate
// on each call; the remaining components are cloned from `&self` unchanged.
// This gives SBFL a healthier pass/fail ratio on shrunk seeds where perturbing
// every component at once almost always breaks the bug condition.
//
// Always-mutate-all is recoverable: for any shape, the "all ones" subset has
// probability 1 / (2^n - 1).

impl<R: Rng, T1: Mutate<R> + Clone, T2: Mutate<R> + Clone> Mutate<R> for (T1, T2) {
    fn mutate(&self, rng: &mut R, n: usize) -> (T1, T2) {
        // 3 non-empty subsets of {1, 2}: 01, 10, 11
        let mask: u8 = rng.random_range(1u8..=3);
        let r1 = if mask & 0b01 != 0 { T1::mutate(&self.0, rng, n) } else { self.0.clone() };
        let r2 = if mask & 0b10 != 0 { T2::mutate(&self.1, rng, n) } else { self.1.clone() };
        (r1, r2)
    }
}

impl<R: Rng, T1: Mutate<R> + Clone, T2: Mutate<R> + Clone, T3: Mutate<R> + Clone> Mutate<R>
    for (T1, T2, T3)
{
    fn mutate(&self, rng: &mut R, n: usize) -> (T1, T2, T3) {
        let mask: u8 = rng.random_range(1u8..=7);
        let r1 = if mask & 0b001 != 0 { T1::mutate(&self.0, rng, n) } else { self.0.clone() };
        let r2 = if mask & 0b010 != 0 { T2::mutate(&self.1, rng, n) } else { self.1.clone() };
        let r3 = if mask & 0b100 != 0 { T3::mutate(&self.2, rng, n) } else { self.2.clone() };
        (r1, r2, r3)
    }
}

impl<
    R: Rng,
    T1: Mutate<R> + Clone,
    T2: Mutate<R> + Clone,
    T3: Mutate<R> + Clone,
    T4: Mutate<R> + Clone,
> Mutate<R> for (T1, T2, T3, T4)
{
    fn mutate(&self, rng: &mut R, n: usize) -> (T1, T2, T3, T4) {
        let mask: u8 = rng.random_range(1u8..=15);
        let r1 = if mask & 0b0001 != 0 { T1::mutate(&self.0, rng, n) } else { self.0.clone() };
        let r2 = if mask & 0b0010 != 0 { T2::mutate(&self.1, rng, n) } else { self.1.clone() };
        let r3 = if mask & 0b0100 != 0 { T3::mutate(&self.2, rng, n) } else { self.2.clone() };
        let r4 = if mask & 0b1000 != 0 { T4::mutate(&self.3, rng, n) } else { self.3.clone() };
        (r1, r2, r3, r4)
    }
}

impl<
    R: Rng,
    T1: Mutate<R> + Clone,
    T2: Mutate<R> + Clone,
    T3: Mutate<R> + Clone,
    T4: Mutate<R> + Clone,
    T5: Mutate<R> + Clone,
> Mutate<R> for (T1, T2, T3, T4, T5)
{
    fn mutate(&self, rng: &mut R, n: usize) -> (T1, T2, T3, T4, T5) {
        let mask: u8 = rng.random_range(1u8..=31);
        let r1 = if mask & 0b00001 != 0 { T1::mutate(&self.0, rng, n) } else { self.0.clone() };
        let r2 = if mask & 0b00010 != 0 { T2::mutate(&self.1, rng, n) } else { self.1.clone() };
        let r3 = if mask & 0b00100 != 0 { T3::mutate(&self.2, rng, n) } else { self.2.clone() };
        let r4 = if mask & 0b01000 != 0 { T4::mutate(&self.3, rng, n) } else { self.3.clone() };
        let r5 = if mask & 0b10000 != 0 { T5::mutate(&self.4, rng, n) } else { self.4.clone() };
        (r1, r2, r3, r4, r5)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ResultStatus {
    /// Exceeds the maximum number of passed tests.
    Finished,
    /// Exceeded maximum number of discards.
    GaveUp,
    /// Exceeded maximum time limit.
    TimedOut,
    /// The test failed with a counterexample.
    Failed { arguments: Vec<String> },
    /// The test was aborted due to an internal error.
    Aborted { error: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunResult {
    /// Status
    pub status: ResultStatus,
    /// The number of tests that passed.
    pub passed: u64,
    /// The number of tests that were discarded.
    pub discarded: u64,
}

pub trait Implies<T> {
    fn implies(self, other: T) -> Option<bool>;
}

impl Implies<bool> for bool {
    fn implies(self, other: bool) -> Option<bool> {
        if self { Some(other) } else { None }
    }
}

impl Implies<bool> for Option<bool> {
    fn implies(self, other: bool) -> Option<bool> {
        match self {
            Some(true) => Some(other),
            Some(false) | None => None,
        }
    }
}

impl Implies<Option<bool>> for bool {
    fn implies(self, other: Option<bool>) -> Option<bool> {
        if self { other } else { None }
    }
}

impl Implies<Option<bool>> for Option<bool> {
    fn implies(self, other: Option<bool>) -> Option<bool> {
        match self {
            Some(true) => other,
            Some(false) | None => None,
        }
    }
}

/// Split a tuple's Debug representation into per-element strings.
///
/// Rust's tuple Debug impl produces `"(a, b, c)"`, where inner commas inside
/// nested brackets must be preserved. This function strips the outer `(...)`
/// and splits on top-level `, ` (bracket-depth-aware), so a tuple like
/// `([1, 2], [(3, 4)])` produces `["[1, 2]", "[(3, 4)]"]` rather than a single
/// element with the whole tuple or a comma-stripped corruption.
///
/// If `s` is not a parenthesized tuple, the whole string is returned as a
/// single-element vec.
fn split_tuple_debug(s: &str) -> Vec<String> {
    if !(s.starts_with('(') && s.ends_with(')') && s.len() >= 2) {
        return vec![s.to_string()];
    }
    let core = &s[1..s.len() - 1];
    let bytes = core.as_bytes();
    let mut parts: Vec<String> = Vec::new();
    let mut depth: i32 = 0;
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b',' if depth == 0 => {
                parts.push(core[start..i].to_string());
                let skip = if i + 1 < bytes.len() && bytes[i + 1] == b' ' { 2 } else { 1 };
                start = i + skip;
                i += skip;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    parts.push(core[start..].to_string());
    // Guard: if depth didn't balance to zero (unbalanced brackets in an element's
    // own Debug output), fall back to the whole string to avoid lossy splits.
    if depth != 0 {
        return vec![s.to_string()];
    }
    // A parenthesized single element (e.g., `(42)`) is a rare case — treat as
    // the single element.
    if parts.len() == 1 {
        return parts;
    }
    parts
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// Maximum number of tests to generate and execute.
    pub tests: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self { tests: 20_000 }
    }
}

pub fn quickcheck<T: Arbitrary<ThreadRng> + Clone + Debug>(f: fn(T) -> Option<bool>) -> RunResult {
    quickcheck_with_config(Config::default(), f)
}

pub fn quickcheck_with_config<T: Arbitrary<ThreadRng> + Clone + Debug>(
    config: Config,
    f: fn(T) -> Option<bool>,
) -> RunResult {
    let mut rng = rand::rng();
    let mut passed = 0;
    let mut discarded = 0;
    for i in 0..config.tests {
        let input = T::generate(&mut rng, ((i + 1) as f32).log2() as usize);
        tracing::trace!("test #{}: {:?}", i + 1, input);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(input.clone())));
        match outcome {
            Ok(None) => discarded += 1,
            Ok(Some(true)) => passed += 1,
            Ok(Some(false)) | Err(_) => {
                return RunResult {
                    status: ResultStatus::Failed {
                        arguments: split_tuple_debug(&format!("{:?}", input)),
                    },
                    passed,
                    discarded,
                };
            },
        }
    }

    RunResult { passed, discarded, status: ResultStatus::Finished }
}

#[cfg(feature = "tracing")]
#[derive(Clone, Copy, Debug)]
pub struct TracedTreeQuickcheckConfig {
    pub tests: u64,
    pub max_size: usize,
    pub max_depth: usize,
}

#[cfg(feature = "tracing")]
impl Default for TracedTreeQuickcheckConfig {
    fn default() -> Self {
        Self { tests: 20_000, max_size: 100, max_depth: 6 }
    }
}

#[cfg(feature = "tracing")]
pub fn quickcheck_traced_tree(
    config: TracedTreeQuickcheckConfig,
    f: fn(crate::tracing::Tree<usize>) -> Option<bool>,
) -> RunResult {
    let mut rng = rand::rng();
    let mut passed = 0;
    let mut discarded = 0;

    for i in 0..config.tests {
        let seed = rng.random::<u64>();
        let size = (((i + 1) as f32).log2() as usize).min(config.max_size);
        let (tree, trace) = match crate::tracing::generate_traced_tree(seed, size, config.max_depth)
        {
            Ok(value) => value,
            Err(err) => {
                return RunResult {
                    status: ResultStatus::Aborted {
                        error: format!("trace generation failed: {:?}", err),
                    },
                    passed,
                    discarded,
                };
            },
        };

        let plain = tree.lift_back();
        tracing::trace!("traced test #{} (seed={}): {:?}", i + 1, seed, plain);
        match f(plain.clone()) {
            None => discarded += 1,
            Some(true) => passed += 1,
            Some(false) => {
                let shrunk_trace = crate::tracing::shrink_traced_tree(
                    seed,
                    size,
                    config.max_depth,
                    trace,
                    |candidate| matches!(f(candidate.clone()), Some(false)),
                );

                let (shrunk_tree, _) = match crate::tracing::replay_traced_tree(
                    seed,
                    size,
                    config.max_depth,
                    &shrunk_trace,
                ) {
                    Ok(value) => value,
                    Err(err) => {
                        return RunResult {
                            status: ResultStatus::Aborted {
                                error: format!("replaying shrunk trace failed: {:?}", err),
                            },
                            passed,
                            discarded,
                        };
                    },
                };

                let shrunk_plain = shrunk_tree.lift_back();
                let root = traced_tree_root(&shrunk_plain);
                return RunResult {
                    status: ResultStatus::Failed {
                        arguments: vec![
                            format!("{:?}", shrunk_plain),
                            format!("root={}", root),
                            format!("seed={}", seed),
                            format!("trace_len={}", shrunk_trace.len()),
                        ],
                    },
                    passed,
                    discarded,
                };
            },
        }
    }

    RunResult { passed, discarded, status: ResultStatus::Finished }
}

#[cfg(feature = "tracing")]
fn traced_tree_root(tree: &crate::tracing::Tree<usize>) -> usize {
    match tree {
        crate::tracing::Tree::Leaf(value) | crate::tracing::Tree::Node(value, _, _) => *value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quickcheck() {
        let result = quickcheck(|x: Vec<i32>| {
            let mut copy = x.clone();
            copy.reverse();
            copy.reverse();
            Some(copy == *x)
        });
        assert_eq!(result.passed, 100);
        assert_eq!(result.discarded, 0);
        assert!(result.status == ResultStatus::Finished);
    }

    #[test]
    fn test_quickcheck_fail() {
        let result = quickcheck(|x: Vec<i32>| {
            let mut copy = x.clone();
            copy.reverse();
            Some(copy == *x)
        });
        assert!(result.passed < 100);
        assert!(result.status == ResultStatus::Failed { arguments: vec![format!("{:?}", result)] });
    }

    #[test]
    fn test_quickcheck_tuple() {
        let result = quickcheck(|(mut x, y): (Vec<i32>, i32)| {
            let len = x.len();
            x.push(y);
            Some(len + 1 == x.len())
        });
        assert_eq!(result.passed, 100);
        assert_eq!(result.discarded, 0);
        assert!(result.status == ResultStatus::Finished);
    }

    #[cfg(feature = "tracing")]
    #[test]
    fn test_quickcheck_traced_tree_returns_shrunk_counterexample() {
        let result = quickcheck_traced_tree(
            TracedTreeQuickcheckConfig { tests: 16, max_size: 20, max_depth: 4 },
            |_tree| Some(false),
        );

        match result.status {
            ResultStatus::Failed { arguments } => {
                assert!(arguments.iter().any(|arg| arg == "trace_len=0"));
                assert!(arguments.iter().any(|arg| arg.starts_with("seed=")));
            },
            _ => panic!("expected traced quickcheck to fail with a shrunk counterexample"),
        }
    }
}
