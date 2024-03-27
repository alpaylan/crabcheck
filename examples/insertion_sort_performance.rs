use crabcheck::{
    fuzzing::maximizing_fuzz_loop,
    targeting::maximizing_targeting_loop,
    utils::with_time,
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

fn instrumented_insertion_sort(array: &mut [i32]) -> usize {
    let mut count = 0;
    for i in 1..array.len() {
        let mut j = i;
        while j > 0 && array[j - 1] > array[j] {
            array.swap(j - 1, j);
            count += 1;
            j -= 1;
        }
    }
    count
}

fn main() {
    let mut seed = maximizing_fuzz_loop(
        |mut input: Vec<i32>| {
            insertion_sort(&mut input);
        },
        with_time,
    );

    println!("Seed: {:?}", seed.input);
    println!("Feedback: {:?}", seed.feedback);

    let comps = instrumented_insertion_sort(&mut seed.input);
    let len = seed.input.len();
    println!("Comps/Len^2: {}", comps as f64 / ((len * len) as f64));

    let seed = maximizing_targeting_loop(
        |mut input: Vec<i32>| {
            insertion_sort(&mut input);
        },
        |input: Vec<i32>, _| {
            let comps = instrumented_insertion_sort(&mut input.clone());
            ((comps as f64 / (input.len() * input.len()) as f64) * 1000000.0) as usize
        },
    );

    println!("Seed: {:?}", seed.input);
    println!("Feedback: {:?}", seed.feedback);
}
