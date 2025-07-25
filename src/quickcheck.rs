use std::fmt::{Debug, Display};

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
        rng.gen_range(-(n as i32)..=(n as i32))
    }
}

impl<R: Rng> Mutate<R> for i32 {
    fn mutate(&self, rng: &mut R, n: usize) -> i32 {
        rng.gen_range(self - 10..self + 10)
    }
}


impl<R: Rng> Arbitrary<R> for Vec<i32> {
    fn generate(rng: &mut R, n: usize) -> Vec<i32> {
        let mut list = Vec::with_capacity(n);
        for _ in 0..n {
            list.push(i32::generate(rng, n));
        }
        list
    }
}

impl<R: Rng> Mutate<R> for Vec<i32> {
    fn mutate(&self, rng: &mut R, n: usize) -> Vec<i32> {
        let mut copy = self.clone();

        // Pick a portion of the list and mutate it
        let a = rng.gen_range(0..=self.len());
        let b = rng.gen_range(a..=self.len());

        for value in copy[a..b].iter_mut() {
            *value = i32::generate(rng, n);
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

impl<R: Rng, T1: Mutate<R>, T2: Mutate<R>> Mutate<R> for (T1, T2) {
    fn mutate(&self, rng: &mut R, n: usize) -> (T1, T2) {
        let r1 = T1::mutate(&self.0, rng, n);
        let r2 = T2::mutate(&self.1, rng, n);
        (r1, r2)
    }
}

#[derive(Default, Debug, Clone)]
pub struct RunResult {
    pub passed: usize,
    pub discarded: usize,
    pub counterexample: Option<String>,
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
    let mut rng = rand::thread_rng();
    let n = 100;
    let mut passed = 0;
    let mut discarded = 0;
    for i in 0..n {
        let input = T::generate(&mut rng, ((i + 1) as f32).log2() as usize);
        match f(input.clone()) {
            None => discarded += 1,
            Some(true) => passed += 1,
            Some(false) => return RunResult { passed, discarded, counterexample: Some(format!("{:?}", input)) },
        }
    }

    RunResult { passed, discarded, counterexample: None }
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
        assert!(result.counterexample.is_none());
    }

    #[test]
    fn test_quickcheck_fail() {
        let result = quickcheck(|x: Vec<i32>| {
            let mut copy = x.clone();
            copy.reverse();
            Some(copy == *x)
        });
        assert!(result.passed < 100);
        assert!(result.counterexample.is_some());
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
        assert!(result.counterexample.is_none());
    }
}
