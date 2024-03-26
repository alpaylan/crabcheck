use rand::Rng;

pub trait Arbitrary {
    fn generate() -> Self;
}

pub trait ArbitrarySized {
    fn generate_sized(n: usize) -> Self;
}

pub trait Mutate {
    fn mutate(&self) -> Self;
}


impl Arbitrary for i32 {
    fn generate() -> i32 {
        let mut rng = rand::thread_rng();
        rng.gen_range(1..100)
    }
}

impl Mutate for i32 {
    fn mutate(&self) -> i32 {
        let mut rng = rand::thread_rng();
        rng.gen_range(self - 10..self + 10)
    }
}


impl Arbitrary for Vec<i32> {
    fn generate() -> Vec<i32> {
        let mut rng = rand::thread_rng();
        let n = rng.gen_range(100..200);
        let mut list = Vec::with_capacity(n);
        for _ in 0..n {
            list.push(i32::generate());
        }
        list
    }
}

impl Mutate for Vec<i32> {
    fn mutate(&self) -> Vec<i32> {
        let mut copy = self.clone();
        
        // Pick a portion of the list and mutate it
        let mut rng = rand::thread_rng();
        let a = rng.gen_range(0..self.len());
        let b = rng.gen_range(a..self.len());

        for value in copy[a..b].iter_mut() {
            *value = i32::generate();
        }

        copy
    }
}


impl<T1: Arbitrary, T2: Arbitrary> Arbitrary for (T1, T2) {
    fn generate() -> (T1, T2) {
        (T1::generate(), T2::generate())
    }
}

impl<T1: Mutate, T2: Mutate> Mutate for (T1, T2) {
    fn mutate(&self) -> (T1, T2) {
        (T1::mutate(&self.0), T2::mutate(&self.1))
    }
}


pub struct RunResult<T> {
    pub passed: usize,
    pub discarded: usize,
    pub counterexample: Option<T>,
}

pub fn quickcheck<T: Arbitrary>(f: fn(&mut T) -> bool) -> RunResult<T> {
    for i in 0..100 {
        let mut input = T::generate();
        match f(&mut input) {
            true => continue,
            false => return RunResult { passed: i, discarded: 0, counterexample: Some(input) },
        }
    }

    RunResult { passed: 100, discarded: 0, counterexample: None }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quickcheck() {
        let result = quickcheck(|x: &mut Vec<i32>| {
            let mut copy = x.clone();
            copy.reverse();
            copy.reverse();
            copy == *x
        });
        assert_eq!(result.passed, 100);
        assert_eq!(result.discarded, 0);
        assert!(result.counterexample.is_none());
    }

    #[test]
    fn test_quickcheck_fail() {
        let result = quickcheck(|x: &mut Vec<i32>| {
            let mut copy = x.clone();
            copy.reverse();
            copy == *x
        });
        assert!(result.passed < 100);
        assert!(result.counterexample.is_some());
    }

    #[test]
    fn test_quickcheck_tuple() {
        let result = quickcheck(|(x, y): &mut (Vec<i32>, i32)| {
            let mut copy = x.clone();
            copy.push(*y);

            copy.len() == x.len() + 1
        });
        assert_eq!(result.passed, 100);
        assert_eq!(result.discarded, 0);
        assert!(result.counterexample.is_none());
    }
}
