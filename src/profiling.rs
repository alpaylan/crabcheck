use std::{
    fmt::Debug,
    fs,
    time::SystemTime,
};

use {
    rand::rngs::ThreadRng,
    serde::Serialize,
};

use crate::quickcheck::{
    Arbitrary,
    Mutate,
    RunResult,
};


extern "C" {
    fn __llvm_profile_write_file() -> i32;
    fn __llvm_profile_reset_counters();
}


pub(crate) fn snapshot(label: &str) {
    unsafe {
        __llvm_profile_write_file();
    }

    // Glob to find the current .profraw file
    let files = glob::glob("target/llvm-cov-target/*.profraw").unwrap();
    let newest: Option<std::path::PathBuf> = files.filter_map(Result::ok).max_by_key(|path| {
        path.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH)
    });

    if let Some(path) = newest {
        let new_name = format!("target/llvm-cov-target/snapshot_{}.profraw", label);
        fs::rename(&path, &new_name).expect("rename failed");
    }
}

pub(crate) fn reset() {
    unsafe { __llvm_profile_reset_counters() };
}

pub fn quickcheck<T: Arbitrary<ThreadRng> + Mutate<ThreadRng> + Clone + Debug>(
    f: fn(T) -> Option<bool>,
) -> RunResult {
    let mut rng = rand::thread_rng();
    let n = 20000;
    let mut passed = 0;
    let mut discarded = 0;
    for i in 0..n {
        let input = T::generate(&mut rng, ((i + 1) as f32).log2() as usize);
        match f(input.clone()) {
            None => discarded += 1,
            Some(true) => passed += 1,
            Some(false) => {
                let (mut positives, mut negatives) = (vec![], vec![(0, format!("{:?}", input))]);
                crate::profiling::reset();
                let _ = f(input.clone());
                crate::profiling::snapshot(format!("iteration_0").as_str());

                for i in 1..=500 {
                    let input = T::mutate(&input, &mut rng, ((i + 1) as f32).log2() as usize);
                    crate::profiling::reset();
                    let result = f(input.clone());
                    crate::profiling::snapshot(format!("iteration_{i}").as_str());
                    match result {
                        None => panic!("invalid mutation!"),
                        Some(true) => {
                            positives.push((i, format!("{:?}", input)));
                        },
                        Some(false) => {
                            negatives.push((i, format!("{:?}", input)));
                        },
                    }
                }
                println!("positives: {}", positives.len());
                println!("negatives: {}", negatives.len());

                #[derive(Serialize)]
                struct Indices {
                    positives: Vec<usize>,
                    negatives: Vec<usize>,
                    positive_examples: Vec<String>,
                    negative_examples: Vec<String>,
                }

                let indices = Indices {
                    positives: positives.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
                    negatives: negatives.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
                    positive_examples: positives.iter().map(|(_, s)| s.clone()).collect::<Vec<_>>(),
                    negative_examples: negatives.iter().map(|(_, s)| s.clone()).collect::<Vec<_>>(),
                };

                let json = serde_json::to_string(&indices).unwrap();
                let file_path = format!("target/llvm-cov-target/indices.json",);
                println!("JSON: {}", json);
                fs::write(file_path, json).expect("Unable to write file");
                println!("JSON written to target/llvm-cov-target/indices.json");


                return RunResult {
                    passed,
                    discarded,
                    counterexample: Some(negatives[0].1.to_string()),
                };
            },
        }
    }

    RunResult { passed: n, discarded: 0, counterexample: None }
}
