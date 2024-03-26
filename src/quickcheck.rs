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

        for i in a..b {
            copy[i] = i32::generate();
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

fn quickcheck<T: Arbitrary>(f: fn(&T) -> bool) -> RunResult<T> {
    for i in 0..100 {
        let input = T::generate();
        match f(&input) {
            true => continue,
            false => {
                return RunResult {
                    passed: i,
                    discarded: 0,
                    counterexample: Some(input),
                }
            }
        }
    }

    RunResult {
        passed: 100,
        discarded: 0,
        counterexample: None,
    }
}

