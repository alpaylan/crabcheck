//! Performance fuzzing with the loop shipped in `crabcheck::loops`: the
//! feedback is how long the property took, and a first-in, first-out
//! queue of seeds rotates through the inputs that were slow. Insertion
//! sort is the subject; the slowest inputs are the long, reversed ones.
//! The output depends on timing, so it is not snapshot-tested.
//!
//! ```text
//! cargo run --release -p crabcheck --example perf_fuzz
//! ```

use crabcheck::{
    Config,
    Run,
    gens,
    loops::{
        self,
        SeedPool,
    },
};

/// Insertion sort, counting the moves it makes.
fn insertion_sort(v: &mut [i32]) -> u64 {
    let mut moves = 0;
    for i in 1..v.len() {
        let mut j = i;
        while j > 0 && v[j - 1] > v[j] {
            v.swap(j - 1, j);
            moves += 1;
            j -= 1;
        }
    }
    moves
}

fn main() {
    let config = Config::default().max_size(64).tests(0).name("perf_fuzz");
    let mut run = Run::new(&config, gens::vec(gens::int(0i32..=999)), |mut v: Vec<i32>| {
        insertion_sort(&mut v);
        v.windows(2).all(|w| w[0] <= w[1])
    });
    let mut queue = loops::Fifo::new();
    loops::perf_fuzz(&mut run, &mut queue, 2_000);
    let slowest = queue.best().map(|seed| (seed.feedback.score, seed.input.clone()));
    print!("{}", run.finish());
    if let Some((micros, input)) = slowest {
        let mut sorted = input.clone();
        let moves = insertion_sort(&mut sorted);
        println!("  slowest input: {micros:.0} µs, {} elements, {moves} moves", input.len());
    }
}
