//! Prints what the in-process coverage reader sees. Build with coverage
//! instrumentation to get real data:
//!
//! ```text
//! RUSTFLAGS="-C instrument-coverage" cargo run -p crabcheck --features coverage --example coverage_probe
//! ```
//!
//! Without the flag it reports that coverage is unavailable and exits.

#[cfg(feature = "coverage")]
fn main() {
    use crabcheck::coverage::{
        Functions,
        available,
        snapshot,
    };

    if !available() {
        println!("coverage unavailable: build with RUSTFLAGS=\"-C instrument-coverage\"");
        return;
    }
    let functions = Functions::get();
    println!(
        "{} counters, {} instrumented functions",
        snapshot().map_or(0, |s| s.len()),
        functions.all().len()
    );
    let before = snapshot().expect("instrumented");
    let mut v: Vec<u32> = (0..50).rev().collect();
    v.sort();
    let after = snapshot().expect("instrumented");
    let delta = before.delta(&after);
    println!("sorting a vector touched {} counters in these functions:", delta.hits().len());
    for function in functions.reached(&delta).take(15) {
        println!("  {}", function.name);
    }
    let own: Vec<_> = functions.in_crate("crabcheck").take(5).map(|f| f.name.clone()).collect();
    println!("first crabcheck functions: {own:?}");
}

#[cfg(not(feature = "coverage"))]
fn main() {
    println!(
        "enable the `coverage` feature: cargo run --features coverage --example coverage_probe"
    );
}
