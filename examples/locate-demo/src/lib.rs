//! A toy library with one planted bug and one property over it.
//!
//! `buggy_addition` panics when `x > 100`. The property says the function
//! agrees with wrapping addition. Random testing trips the panic, shrinking
//! finds a minimal input, and fault localization ranks `buggy_addition` first
//! from the coverage of passing and failing runs plus the panic site.

use crabcheck::{
    Arbitrary,
    Config,
    Report,
    any,
    check,
    gens,
};

/// Adds two numbers, badly.
pub fn buggy_addition(x: i32, y: i32) -> i32 {
    if x > 100 {
        panic!("planted overflow at x > 100");
    }
    x.wrapping_add(y)
}

/// Adds two numbers, correctly; here so the crate has more than one function
/// to rank.
pub fn honest_addition(x: i32, y: i32) -> i32 {
    x.wrapping_add(y)
}

/// A pair of small integers; a quarter of them trip the bug.
#[derive(Clone, Debug, Arbitrary)]
pub struct Pair {
    #[crabcheck(gen = gens::int(-200..=200))]
    pub x: i32,
    #[crabcheck(gen = gens::int(-200..=200))]
    pub y: i32,
}

/// The property: the buggy and the honest addition agree.
pub fn addition_is_total(p: Pair) -> bool {
    buggy_addition(p.x, p.y) == honest_addition(p.x, p.y)
}

/// Run the property with localization on and return the report.
pub fn check_addition() -> Report {
    let mut config = Config::default().name("addition_is_total").tests(1000);
    // The property may be called from an integration test or a binary; the
    // code under test lives in this library.
    config.coverage.filter = "locate_demo".to_string();
    check(&config, any::<Pair>(), addition_is_total)
}

#[cfg(test)]
mod tests {
    #[test]
    fn finds_the_planted_bug() {
        let report = super::check_addition();
        eprintln!("{report}");
        let top = &report.locate.as_ref().expect("localization ran").suspects[0];
        assert!(top.function.contains("buggy_addition"), "{top:?}");
    }
}
