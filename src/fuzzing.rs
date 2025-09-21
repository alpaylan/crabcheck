use rand::rngs::ThreadRng;

use crate::quickcheck::ResultStatus;

use {
    crate::{
        quickcheck::{
            Arbitrary,
            Mutate,
            RunResult,
        },
        seedpool::{
            Seed,
            SeedPool,
        },
    },
    std::fmt::Debug,
};

pub fn maximizing_fuzz_loop<
    Domain: Clone + Arbitrary<ThreadRng> + Mutate<ThreadRng>,
    Codomain,
    Feedback: Clone + Ord + Debug,
>(
    f: fn(Domain) -> Codomain,
    fb: fn(Box<dyn FnOnce() -> Codomain + '_>) -> (Codomain, Feedback),
) -> Seed<Domain, Feedback> {
    let mut pool: SeedPool<Domain, Feedback> = SeedPool::new();
    let fuel = 1000;
    let mut rng = rand::rng();

    for i in 1..=fuel {
        if i % 1000 == 0 {
            println!("Iteration: {}", i);
            println!("Pool size: {}", pool.size());
            println!("Best of all time: {:?}", pool.best_of_all_time.clone().unwrap().feedback);
            println!("====================\n");
        }
        let input = if let Some(seed) = pool.pop() {
            Domain::mutate(&seed.input, &mut rng, (i as f32).log2() as usize)
        } else {
            Domain::generate(&mut rng, (i as f32).log2() as usize)
        };

        let copy = input.clone();
        let (_, feedback) = fb(Box::new(move || f(copy)));

        if pool.is_empty() || feedback > pool.best().clone().feedback {
            let seed = Seed { input, feedback, energy: 1000 };
            pool.add_seed(seed);
        }

        #[cfg(feature = "profiling")]
        if std::env::var("SNAPSHOT").is_ok() {
            crate::profiling::snapshot(format!("iteration_{i}").as_str());
            crate::profiling::reset();
        }
    }

    pool.best_of_all_time.unwrap().clone()
}


pub fn prop_fuzz_loop<
    Domain: Clone + Debug + Arbitrary<ThreadRng> + Mutate<ThreadRng>,
    Feedback: Clone + Ord + Debug,
>(
    p: fn(Domain) -> bool,
    fb: fn(Box<dyn FnOnce() -> bool + '_>) -> (bool, Feedback),
) -> RunResult {
    let mut pool: SeedPool<Domain, Feedback> = SeedPool::new();
    let fuel = 1000;
    let mut rng = rand::rng();

    for i in 1..=fuel {
        if i % 1000 == 0 {
            println!("Iteration: {}", i);
            println!("Pool size: {}", pool.size());
            println!("Best of all time: {:?}", pool.best_of_all_time.clone().unwrap().feedback);
            println!("====================\n");
        }
        let input = if let Some(seed) = pool.pop() {
            Domain::mutate(&seed.input, &mut rng, (i as f32).log2() as usize)
        } else {
            Domain::generate(&mut rng, (i as f32).log2() as usize)
        };

        let copy = input.clone();
        let (result, feedback) = fb(Box::new(move || p(copy)));

        if !result {
            return RunResult {
                passed: i,
                discarded: 0,
                status: ResultStatus::Failed { arguments: vec![format!("{:?}", input)] },
            };
        }
        if pool.is_empty() || feedback > pool.best().clone().feedback {
            let seed = Seed { input, feedback, energy: 1000 };
            pool.add_seed(seed);
        }
    }

    RunResult { passed: fuel, discarded: 0, status: ResultStatus::Finished }
}
