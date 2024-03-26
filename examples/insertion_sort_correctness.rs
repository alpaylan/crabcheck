use std::borrow::BorrowMut;

use crabcheck::quickcheck::quickcheck;

fn insertion_sort(arr: &mut [i32]) {
    for i in 1..arr.len() {
        let mut j = i;
        while j > 0 && arr[j - 1] > arr[j] {
            arr.swap(j - 1, j);
            j -= 1;
        }
    }
}

fn insertion_sort_false(arr: &mut [i32]) {
    for i in 1..arr.len() - 1 {
        let mut j = i;
        while j > 0 && arr[j - 1] > arr[j] {
            arr.swap(j - 1, j);
            j -= 1;
        }
    }
}

fn is_sorted(arr: &[i32]) -> bool {
    for i in 1..arr.len() {
        if arr[i - 1] > arr[i] {
            return false;
        }
    }
    true
}

fn main() {
    let result = quickcheck(|input: &mut Vec<i32>| {
        insertion_sort(input);
        is_sorted(input)
    });

    assert!(result.counterexample.is_none());

    let result = quickcheck(|input: &mut Vec<i32>| {
        insertion_sort_false(input);
        is_sorted(input)
    });

    assert!(result.counterexample.is_some());
    println!("Counterexample: {:?}", result.counterexample);
}
