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
    Domain: Clone + Arbitrary + Mutate,
    Codomain,
    Feedback: Clone + Ord + Debug,
>(
    f: fn(Domain) -> Codomain,
    fb: fn(Box<dyn FnOnce() -> Codomain + '_>) -> (Codomain, Feedback),
) -> Seed<Domain, Feedback> {
    let mut pool: SeedPool<Domain, Feedback> = SeedPool::new();
    let fuel = 100000;

    for i in 1..=fuel {
        if i % 1000 == 0 {
            println!("Iteration: {}", i);
            println!("Pool size: {}", pool.size());
            println!("Best of all time: {:?}", pool.best_of_all_time.clone().unwrap().feedback);
            println!("====================\n");
        }
        let input = if let Some(seed) = pool.pop() {
            Domain::mutate(&seed.input)
        } else {
            Domain::generate()
        };

        let copy = input.clone();
        let (_, feedback) = fb(Box::new(move || f(copy)));

        if pool.is_empty() || feedback > pool.best().clone().feedback {
            let seed = Seed { input, feedback, energy: 1000 };
            pool.add_seed(seed);
        }
    }

    pool.best_of_all_time.unwrap().clone()
}


pub fn prop_fuzz_loop<Domain: Clone + Arbitrary + Mutate, Feedback: Clone + Ord + Debug>(
    p: fn(Domain) -> bool,
    fb: fn(Box<dyn FnOnce() -> bool + '_>) -> (bool, Feedback),
) -> RunResult<Seed<Domain, Feedback>> {
    let mut pool: SeedPool<Domain, Feedback> = SeedPool::new();
    let fuel = 100000;

    for i in 1..=fuel {
        if i % 1000 == 0 {
            println!("Iteration: {}", i);
            println!("Pool size: {}", pool.size());
            println!("Best of all time: {:?}", pool.best_of_all_time.clone().unwrap().feedback);
            println!("====================\n");
        }
        let input = if let Some(seed) = pool.pop() {
            Domain::mutate(&seed.input)
        } else {
            Domain::generate()
        };

        let copy = input.clone();
        let (result, feedback) = fb(Box::new(move || p(copy)));

        if !result {
            return RunResult {
                passed: i,
                discarded: 0,
                counterexample: Some(Seed { input, feedback, energy: 1000 }),
            };
        }
        if pool.is_empty() || feedback > pool.best().clone().feedback {
            let seed = Seed { input, feedback, energy: 1000 };
            pool.add_seed(seed);
        }
    }

    RunResult { passed: fuel, discarded: 0, counterexample: None }
}
