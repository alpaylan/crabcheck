//! A testing loop written by hand from the same steps `check` is made of.
//! Random tests first, exactly as `check` runs them; then, instead of the
//! built-in targeted phase, climb the score by mutating the best seeds and
//! stop after a run of rounds without progress.
//!
//! ```text
//! cargo run -p crabcheck --example custom_loop
//! ```

use crabcheck::{
    Config,
    Run,
    Status,
    gens,
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

fn main() {
    let config = Config::default().max_size(12).tests(300).name("custom_loop");
    let inputs = gens::vec(gens::int(0i32..=99));
    let mut run = Run::new(&config, inputs, |mut v: Vec<i32>| {
        let n = v.len() as u64;
        let worst = n * n.saturating_sub(1) / 2;
        let moves = insertion_sort(&mut v);
        if worst > 0 {
            target(moves as f64 / worst as f64);
        }
        v.windows(2).all(|w| w[0] <= w[1]) && moves <= worst
    });

    // Phase 1: random testing, as `check` does it.
    let mut best = 0.0f64;
    while run.wants_more() {
        let Some(input) = run.draw() else { continue };
        let tested = run.test(&input);
        if tested.passed() {
            best = best.max(tested.score().unwrap_or(0.0));
            run.keep(input.value, &tested);
        }
    }

    // Phase 2: climb the score with patience instead of a fixed budget.
    let mut stale = 0;
    while stale < 40 {
        let Some(parent) = run.pool_pick() else { break };
        let Some(child) = run.mutate(&parent) else {
            if run.report().status != Status::Passed {
                break;
            }
            continue;
        };
        let tested = run.test(&child);
        if tested.failed() {
            break;
        }
        match tested.score() {
            Some(score) if score > best => {
                best = score;
                stale = 0;
                run.keep_always(child.value, score);
            },
            _ => stale += 1,
        }
    }

    let report = run.finish();
    print!("{report}");
    report.assert_passed();
}
