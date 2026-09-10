//! A loop shipped in `crabcheck::loops` with a seed pool policy: targeted
//! testing with a heap that always mutates the best seed, on insertion
//! sort with its number of moves as the score. `perf_fuzz.rs` runs the
//! performance loop on the same property.
//!
//! ```text
//! cargo run -p crabcheck --example shipped_loops
//! ```

use crabcheck::{
    Config,
    Run,
    gens,
    loops::{
        self,
        SeedPool,
    },
    target,
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

fn sorts(mut v: Vec<i32>) -> bool {
    let moves = insertion_sort(&mut v);
    target(moves as f64);
    v.windows(2).all(|w| w[0] <= w[1])
}

fn main() {
    let inputs = gens::vec(gens::int(0i32..=99));

    // Targeted: the score is the number of moves, and the heap mutates the
    // input with the most moves until its energy is spent.
    let config = Config::default().max_size(12).tests(0).name("targeted");
    let mut run = Run::new(&config, inputs.clone(), sorts);
    let mut heap = loops::Heap::new();
    loops::target(&mut run, &mut heap, 400);
    let best = heap.best().map(|seed| (seed.feedback.score, seed.input.clone()));
    print!("{}", run.finish());
    if let Some((moves, input)) = best {
        println!("  most moves: {moves} by {input:?}");
    }
}
