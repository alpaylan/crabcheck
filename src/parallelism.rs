use std::{
    fmt::Debug,
    sync::{
        atomic::AtomicBool,
        Arc,
        Mutex,
    },
};

use crate::quickcheck::{
    Arbitrary,
    RunResult,
};

pub fn par_quickcheck<T: Arbitrary + Sync + Send + Debug + Clone + 'static>(
    f: fn(&mut T) -> bool,
) -> RunResult<T> {
    let result: Arc<Mutex<Option<RunResult<T>>>> = Arc::new(Mutex::new(None));
    let done = Arc::new(AtomicBool::new(false));

    let mut threads = vec![];

    for _ in 0..4 {
        let done = done.clone();
        let result = result.clone();
        let thread = std::thread::spawn(move || {
            for i in 0..100 {
                if done.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }

                let mut input = T::generate();
                match f(&mut input) {
                    true => continue,
                    false => {
                        let mut result = result.lock().unwrap();
                        *result = Some(RunResult {
                            passed: i,
                            discarded: 0,
                            counterexample: Some(input),
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
        None => RunResult { passed: 100, discarded: 0, counterexample: None },
    }
}

#[cfg(test)]
mod tests {
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
        assert!(result.counterexample.is_none());
    }

    #[test]
    fn test_quickcheck_fail() {
        let result = par_quickcheck(|x: &mut Vec<i32>| {
            let mut copy = x.clone();
            copy.reverse();
            copy == *x
        });
        assert!(result.passed < 100);
        assert!(result.counterexample.is_some());
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
        assert!(result.counterexample.is_none());
    }
}
