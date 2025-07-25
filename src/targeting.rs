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
    rand::rngs::ThreadRng,
    std::fmt::Debug,
};

pub fn maximizing_targeting_loop<
    Domain: Clone + Debug + Arbitrary<ThreadRng> + Mutate<ThreadRng>,
    Codomain,
    Feedback: Clone + Ord + Debug,
>(
    f: fn(Domain) -> Codomain,
    fb: fn(Domain, Codomain) -> Feedback,
) -> Seed<Domain, Feedback> {
    let mut pool: SeedPool<Domain, Feedback> = SeedPool::new();
    let fuel = 1000;
    let mut rng = rand::thread_rng();

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

        let result = f(input.clone());
        let feedback = fb(input.clone(), result);

        if pool.is_empty() || feedback > pool.best().clone().feedback {
            let seed = Seed { input, feedback, energy: 1000 };
            pool.add_seed(seed);
        }
    }

    pool.best_of_all_time.unwrap().clone()
}

pub fn prop_targeting_loop<
    Domain: Clone + Debug + Arbitrary<ThreadRng> + Mutate<ThreadRng>,
    Feedback: Clone + Ord + Debug,
>(
    f: fn(Domain) -> bool,
    fb: fn(Domain) -> Feedback,
) -> RunResult {
    let mut pool: SeedPool<Domain, Feedback> = SeedPool::new();
    let fuel = 100000;
    let mut rng = rand::thread_rng();

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

        let result = f(input.clone());
        let feedback = fb(input.clone());

        if !result {
            return RunResult {
                passed: i,
                discarded: 0,
                counterexample: Some(format!("{:?}", input)),
            };
        }

        if pool.is_empty() || feedback > pool.best().clone().feedback {
            let seed = Seed { input, feedback, energy: 1000 };
            pool.add_seed(seed);
        }
    }

    RunResult { passed: fuel, discarded: 0, counterexample: None }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_maximizing_targeting_loop() {
        let result =
            maximizing_targeting_loop(|x: Vec<i32>| x.iter().sum(), |_x: Vec<i32>, y: i32| y);

        let avg: i32 = <Vec<i32>>::generate(&mut rand::thread_rng(), 100).iter().sum();

        assert!(result.feedback > avg);
    }
}
