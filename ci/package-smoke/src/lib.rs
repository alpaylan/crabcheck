//! A consumer of the packaged crates, built by `ci/package-smoke.sh` against
//! the `.crate` contents the way a crates.io user would see them.

use crabcheck::{
    Arbitrary,
    gens,
};

/// A closed interval on 0..=100, not necessarily ordered.
#[derive(Clone, Debug, Arbitrary)]
pub struct Interval {
    #[crabcheck(gen = gens::int(0i32..=100))]
    pub lo: i32,
    #[crabcheck(gen = gens::int(0i32..=100))]
    pub hi: i32,
}

impl Interval {
    /// The distance between the ends.
    pub fn width(&self) -> i32 {
        (self.hi - self.lo).abs()
    }
}
