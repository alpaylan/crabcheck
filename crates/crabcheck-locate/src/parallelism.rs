use std::{
    fmt::Debug,
    sync::{
        Arc,
        Mutex,
        atomic::AtomicBool,
    },
};

use rand::rngs::ThreadRng;

use crate::quickcheck::{
    Arbitrary,
    ResultStatus,
    RunResult,
};

pub fn par_quickcheck<T: Arbitrary<ThreadRng> + Sync + Send + Debug + Clone + 'static>(
    f: fn(&mut T) -> bool,
) -> RunResult {
    let result: Arc<Mutex<Option<RunResult>>> = Arc::new(Mutex::new(None));
    let done = Arc::new(AtomicBool::new(false));
    let mut threads = vec![];

    for _ in 0..4 {
        let done = done.clone();
        let result = result.clone();
        let thread = std::thread::spawn(move || {
            let mut rng = rand::rng();
            for i in 0..100 {
                if done.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }

                let mut input = T::generate(&mut rng, ((i + 1) as f32).log2() as usize);
                match f(&mut input) {
                    true => continue,
                    false => {
                        let mut result = result.lock().unwrap();
                        *result = Some(RunResult {
                            status: ResultStatus::Failed {
                                arguments: vec![format!("{:?}", input)],
                            },
                            passed: i,
                            discarded: 0,
                        });
                        done.store(true, std::sync::atomic::Ordering::Relaxed);
                        return;
                    },
                }
            }
        });
        threads.push(thread);
    }

    for thread in threads {
        thread.join().unwrap();
    }

    let result = result.lock().unwrap();
    match &*result {
        Some(result) => result.clone(),
        None => RunResult { passed: 100, discarded: 0, status: ResultStatus::Finished },
    }
}

#[cfg(test)]
mod tests {
    use crate::quickcheck::ResultStatus;

    use super::*;

    #[test]
    fn test_par_quickcheck() {
        let result = par_quickcheck(|x: &mut Vec<i32>| {
            let mut copy = x.clone();
            copy.reverse();
            copy.reverse();
            copy == *x
        });
        assert_eq!(result.passed, 100);
        assert_eq!(result.discarded, 0);
        assert!(result.status == ResultStatus::Finished);
    }

    #[test]
    fn test_quickcheck_fail() {
        let result = par_quickcheck(|x: &mut Vec<i32>| {
            let mut copy = x.clone();
            copy.reverse();
            copy == *x
        });
        assert!(result.passed < 100);
        assert!(matches!(result.status, ResultStatus::Failed { .. }));
    }

    #[test]
    fn test_quickcheck_tuple() {
        let result = par_quickcheck(|(x, y): &mut (Vec<i32>, i32)| {
            let mut copy = x.clone();
            copy.push(*y);

            copy.len() == x.len() + 1
        });
        assert_eq!(result.passed, 100);
        assert_eq!(result.discarded, 0);
        assert!(result.status == ResultStatus::Finished);
    }
}
