use crabcheck::quickcheck::{
    ResultStatus,
    quickcheck,
};

fn correct_insertion_sort(array: &mut [i32]) {
    for i in 1..array.len() {
        let mut j = i;
        while j > 0 && array[j - 1] > array[j] {
            array.swap(j - 1, j);
            j -= 1;
        }
    }
}

fn bugged_insertion_sort(array: &mut [i32]) {
    for i in 1..array.len() - 1 {
        let mut j = i;
        while j > 0 && array[j - 1] > array[j] {
            array.swap(j - 1, j);
            j -= 1;
        }
    }
}

fn is_sorted(array: &[i32]) -> bool {
    array.windows(2).all(|window| window[0] <= window[1])
}

fn main() {
    let should_work = quickcheck(|mut input: Vec<i32>| {
        correct_insertion_sort(&mut input);
        Some(is_sorted(&input))
    });
    assert!(matches!(should_work.status, ResultStatus::Finished));

    let should_fail = quickcheck(|mut input: Vec<i32>| {
        bugged_insertion_sort(&mut input);
        Some(is_sorted(&mut input))
    });
    assert!(matches!(should_fail.status, ResultStatus::Failed { .. }));
}
