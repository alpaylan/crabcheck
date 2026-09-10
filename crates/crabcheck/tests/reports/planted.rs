//! Fixtures for the instrumented snapshots. Kept in their own file so the
//! line numbers in the suspect list only move when this file changes.

/// Classifies an integer, with a planted panic on large values.
pub fn classify(x: i32) -> &'static str {
    if x < 0 {
        "negative"
    } else if x == 0 {
        "zero"
    } else if x > 1000 {
        panic!("planted: too large")
    } else {
        "positive"
    }
}

/// Never called by any scenario; shows up as never reached.
#[allow(dead_code)]
pub fn never_called(x: i32) -> i32 {
    x.wrapping_mul(3)
}
