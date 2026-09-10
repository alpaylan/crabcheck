//! The classic first property test: sorting. Run with
//! `cargo run -p crabcheck --example insertion_sort`.
//!
//! The correct implementation passes; the buggy one (which skips the last
//! element) fails, and the report shows the input that exposed it.

use crabcheck::{
    Config,
    check,
    classify,
    gens,
};

fn insertion_sort(array: &mut [i32]) {
    for i in 1..array.len() {
        let mut j = i;
        while j > 0 && array[j - 1] > array[j] {
            array.swap(j - 1, j);
            j -= 1;
        }
    }
}

fn buggy_insertion_sort(array: &mut [i32]) {
    for i in 1..array.len().saturating_sub(1) {
        let mut j = i;
        while j > 0 && array[j - 1] > array[j] {
            array.swap(j - 1, j);
            j -= 1;
        }
    }
}

fn is_sorted(array: &[i32]) -> bool {
    array.windows(2).all(|w| w[0] <= w[1])
}

fn main() {
    let config = Config::default().name("insertion_sort");
    let inputs = || gens::vec(gens::int(-1000i32..=1000));

    let correct = check(&config, inputs(), |mut v: Vec<i32>| {
        classify(v.is_empty(), "empty");
        classify(v.len() > 10, "long");
        let mut expected = v.clone();
        expected.sort();
        insertion_sort(&mut v);
        is_sorted(&v) && v == expected
    });
    print!("{correct}");
    assert!(correct.passed());

    let buggy = check(&config, inputs(), |mut v: Vec<i32>| {
        buggy_insertion_sort(&mut v);
        is_sorted(&v)
    });
    print!("{buggy}");
    assert!(!buggy.passed());
}
