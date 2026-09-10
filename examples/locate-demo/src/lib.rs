//! A toy library with one buggy function and one property over it.
//!
//! The bug: `buggy_addition` panics when `x > 100`. The property checks that
//! the result equals plain wrapping addition for any inputs. Crabcheck's
//! random search trips the panic; `quickcheck_with_locate!` then attributes
//! the failure to the `if x > 100` line via the crash-stack prior.

use crabcheck_locate::quickcheck::{Arbitrary, Mutate};
use rand::Rng;

pub fn buggy_addition(x: i32, y: i32) -> i32 {
    if x > 100 {
        panic!("planted overflow at x > 100");
    }
    x.wrapping_add(y)
}

#[derive(Clone, Debug)]
pub struct Pair {
    pub x: i32,
    pub y: i32,
}

impl<R: Rng> Arbitrary<R> for Pair {
    fn generate(rng: &mut R, _n: usize) -> Self {
        Pair { x: rng.random_range(-200..=200), y: rng.random_range(-200..=200) }
    }
}

impl<R: Rng> Mutate<R> for Pair {
    fn mutate(&self, rng: &mut R, _n: usize) -> Self {
        let mut p = self.clone();
        if rng.random::<bool>() {
            p.x = p.x.wrapping_add(rng.random_range(-32..=32));
        } else {
            p.y = p.y.wrapping_add(rng.random_range(-32..=32));
        }
        p
    }
}

/// `Some(true)` when the property holds for the input, `Some(false)` when
/// it's violated, `None` to discard. Crabcheck's profiling runner classifies
/// a panic from inside `f` as `Some(false)` (failure) and records the
/// panic's file:line in `coverage/<workdir>/panic_locations.jsonl`.
pub fn property_addition_total(p: Pair) -> Option<bool> {
    Some(buggy_addition(p.x, p.y) == p.x.wrapping_add(p.y))
}

#[cfg(test)]
mod tests {
    //! Unit tests living *inside* `src/lib.rs` are compiled as part of the
    //! library's test binary (crate name = the package's lib name —
    //! `locate_demo` here). That means `env!("CARGO_CRATE_NAME")` resolves
    //! to `locate_demo` in this scope and the 1-arg macro form works.
    //! This is the pattern most users reach for first.
    use super::*;

    #[test]
    fn unit_test_finds_planted_overflow_bug() {
        let report = crabcheck_locate::quickcheck_with_locate!(property_addition_total);
        eprintln!("[unit_test] {report}");
        report.assert_failed();
        let top = report.top().expect("expected at least one suspect");
        assert!(
            top.region.function.starts_with("locate_demo::buggy_addition"),
            "expected top suspect to be locate_demo::buggy_addition; got {}",
            top.region.function
        );
        assert!(
            top.region.file.ends_with("src/lib.rs"),
            "top suspect should be in src/lib.rs; got {}",
            top.region.file
        );
    }
}
