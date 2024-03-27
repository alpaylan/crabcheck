# Crabcheck

A WIP property-based testing library in Rust, built with generalized targeted-property testing in
mind.

## What is property-based testing?

TODO

## Example usage

```rust
use crabcheck::quickcheck::quickcheck;

fn correct_insertion_sort(arr: &mut [i32]) {
    for i in 1..arr.len() {
        let mut j = i;
        while j > 0 && arr[j - 1] > arr[j] {
            arr.swap(j - 1, j);
            j -= 1;
        }
    }
}

fn bugged_insertion_sort(arr: &mut [i32]) {
    for i in 1..arr.len() - 1 {
        let mut j = i;
        while j > 0 && arr[j - 1] > arr[j] {
            arr.swap(j - 1, j);
            j -= 1;
        }
    }
}

fn is_sorted(array: &[i32]) -> bool {
    array.windows(2).all(|window| window[0] <= window[1])
}

fn main() {
    let should_work = quickcheck(|input: &mut Vec<i32>| {
        correct_insertion_sort(input);
        is_sorted(input)
    });
    assert!(should_work.counterexample.is_none());

    let should_fail = quickcheck(|input: &mut Vec<i32>| {
        bugged_insertion_sort(input);
        is_sorted(input)
    });
    assert!(should_fail.counterexample.is_some());

    println!(
        "Counterexample that doesn't work on buggy implementation: {:?}",
        should_fail.counterexample.unwrap(),
    );
}
```
