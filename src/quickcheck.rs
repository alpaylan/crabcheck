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
        rng.random_range((*self - 10)..=(*self + 10))
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

impl<R: Rng, T1: Mutate<R>, T2: Mutate<R>> Mutate<R> for (T1, T2) {
    fn mutate(&self, rng: &mut R, n: usize) -> (T1, T2) {
        let r1 = T1::mutate(&self.0, rng, n);
        let r2 = T2::mutate(&self.1, rng, n);
        (r1, r2)
    }
}

impl<R: Rng, T1: Mutate<R>, T2: Mutate<R>, T3: Mutate<R>> Mutate<R> for (T1, T2, T3) {
    fn mutate(&self, rng: &mut R, n: usize) -> (T1, T2, T3) {
        // todo: make this a splittable Rng
        let r1 = T1::mutate(&self.0, rng, n);
        let r2 = T2::mutate(&self.1, rng, n);
        let r3 = T3::mutate(&self.2, rng, n);
        (r1, r2, r3)
    }
}

impl<R: Rng, T1: Mutate<R>, T2: Mutate<R>, T3: Mutate<R>, T4: Mutate<R>> Mutate<R>
    for (T1, T2, T3, T4)
{
    fn mutate(&self, rng: &mut R, n: usize) -> (T1, T2, T3, T4) {
        // todo: make this a splittable Rng
        let r1 = T1::mutate(&self.0, rng, n);
        let r2 = T2::mutate(&self.1, rng, n);
        let r3 = T3::mutate(&self.2, rng, n);
        let r4 = T4::mutate(&self.3, rng, n);
        (r1, r2, r3, r4)
    }
}

impl<R: Rng, T1: Mutate<R>, T2: Mutate<R>, T3: Mutate<R>, T4: Mutate<R>, T5: Mutate<R>> Mutate<R>
    for (T1, T2, T3, T4, T5)
{
    fn mutate(&self, rng: &mut R, n: usize) -> (T1, T2, T3, T4, T5) {
        // todo: make this a splittable Rng
        let r1 = T1::mutate(&self.0, rng, n);
        let r2 = T2::mutate(&self.1, rng, n);
        let r3 = T3::mutate(&self.2, rng, n);
        let r4 = T4::mutate(&self.3, rng, n);
        let r5 = T5::mutate(&self.4, rng, n);
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

pub fn quickcheck<T: Arbitrary<ThreadRng> + Clone + Debug>(f: fn(T) -> Option<bool>) -> RunResult {
    let mut rng = rand::rng();
    let n = 20_000;
    let mut passed = 0;
    let mut discarded = 0;
    for i in 0..n {
        let input = T::generate(&mut rng, ((i + 1) as f32).log2() as usize);
        tracing::trace!("test #{}: {:?}", i + 1, input);
        match f(input.clone()) {
            None => discarded += 1,
            Some(true) => passed += 1,
            Some(false) => {
                return RunResult {
                    status: ResultStatus::Failed { arguments: vec![format!("{:?}", input)] },
                    passed,
                    discarded,
                };
            },
        }
    }

    RunResult { passed, discarded, status: ResultStatus::Finished }
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
}
