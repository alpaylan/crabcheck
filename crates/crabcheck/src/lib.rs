//! Property-based testing for Rust, built for tests that are written and
//! verified by coding agents: you author the generator, the support
//! predicate, the shrinker and the mutator, and the library verifies all
//! of them and makes every failure reproducible from its seed.
//!
//! # Overview
//!
//! - [`Source`] is a seeded random source; [`Gen`] is anything that
//!   produces a value from it, together with its shrinker and mutator.
//!   [`gens`] holds the constructors and combinators, all uniform unless a
//!   bias is composed explicitly. [`Arbitrary`] is a type's default
//!   generator (`#[derive(Arbitrary)]` with the `derive` feature).
//! - [`check`] runs a property, shrinks the first failure with the
//!   generator's shrinker, remembers it in a regressions file and returns a
//!   [`Report`] with a replay point. [`check_par`] spreads the random phase
//!   over threads. [`quickcheck`] is `check` with defaults.
//! - [`Shrink`] and [`Mutate`] are the type-level hooks; the built-in
//!   generators carry their own. [`check_generator`], [`check_shrinker`] and
//!   [`check_mutator`] verify hooks against a support predicate.
//! - [`Config`] holds every knob; `CRABCHECK_*` variables override it (see
//!   [`config`]).
//! - Inside a property, [`label`] and [`classify`] record how inputs are
//!   distributed and [`target`] steers the search toward high scores.
//!
//! 0.2 is a rewrite of the 0.1 prototype and its API is not yet frozen.
//!
//! # Checking a property
//!
//! ```
//! use crabcheck::{check, gens, Config};
//!
//! let report = check(&Config::default().tests(500), gens::vec(gens::int(-50i32..=50)), |mut v| {
//!     let before = v.len();
//!     v.sort();
//!     v.len() == before && v.windows(2).all(|w| w[0] <= w[1])
//! });
//! report.assert_passed();
//! ```
//!
//! A property may return `bool`, `()`, `Result<_, E: Debug>` or
//! [`TestResult`]; a panic counts as a failure and its message and location
//! go in the report.
//!
//! # Writing a generator
//!
//! Any `Fn(&mut Source) -> T` is a generator. Draw decisions from the
//! [`Source`] and compose sub-generators with [`Source::draw`]:
//!
//! ```
//! use crabcheck::{Gen, GenExt, Source};
//!
//! #[derive(Debug, Clone, PartialEq)]
//! enum Tree { Leaf, Node(Box<Tree>, i32, Box<Tree>) }
//!
//! fn bst(g: &mut Source, lo: i32, hi: i32, depth: usize) -> Tree {
//!     if depth == 0 || lo > hi || !g.bool_with(0.8) {
//!         return Tree::Leaf;
//!     }
//!     let key = g.int(lo..=hi);
//!     let left = bst(g, lo, key - 1, depth - 1);
//!     let right = bst(g, key + 1, hi, depth - 1);
//!     Tree::Node(Box::new(left), key, Box::new(right))
//! }
//!
//! // A shrinker that keeps the tree a BST: drop a subtree, or replace the
//! // node by one of its children.
//! fn shrink_bst(t: &Tree) -> Vec<Tree> {
//!     match t {
//!         Tree::Leaf => Vec::new(),
//!         Tree::Node(l, k, r) => {
//!             let mut out = vec![(**l).clone(), (**r).clone(), Tree::Leaf];
//!             out.extend(shrink_bst(l).into_iter().map(|l| Tree::Node(Box::new(l), *k, r.clone())));
//!             out.extend(shrink_bst(r).into_iter().map(|r| Tree::Node(l.clone(), *k, Box::new(r))));
//!             out
//!         }
//!     }
//! }
//!
//! let trees = (|g: &mut Source| bst(g, -100, 100, 5)).with_shrink(shrink_bst);
//! let mut source = Source::new(42, 10);
//! let tree = trees.generate(&mut source);
//! // The same seed regenerates the same value.
//! assert_eq!(trees.generate(&mut Source::new(42, 10)), tree);
//! ```

#![forbid(unsafe_code)]

pub mod arbitrary;
pub mod config;
pub mod gens;
mod health;
pub mod hooks;
mod pool;
mod primitives;
#[cfg(feature = "json")]
pub mod regressions;
pub mod report;
pub mod runner;
mod shrink;
pub mod source;
mod verify;

pub use {
    arbitrary::{
        Arbitrary,
        any,
    },
    config::{
        Config,
        ReplayPoint,
    },
    gens::{
        BoxGen,
        Gen,
        GenExt,
    },
    hooks::{
        Mutate,
        Shrink,
    },
    report::{
        Report,
        ShrinkStats,
        Status,
        TargetSummary,
    },
    runner::{
        Outcome,
        TestResult,
        Testable,
        check,
        check_par,
        labels::{
            classify,
            label,
            target,
        },
        quickcheck,
    },
    source::Source,
    verify::{
        check_generator,
        check_mutator,
        check_shrinker,
    },
};

/// Derive a default generator for a struct or enum. Needs the `derive`
/// feature (on by default). See [`crabcheck_macros::Arbitrary`].
#[cfg(feature = "derive")]
pub use crabcheck_macros::Arbitrary;
/// Turn a function with typed parameters into a property-based `#[test]`.
/// Needs the `derive` feature (on by default). See [`crabcheck_macros::test`].
#[cfg(feature = "derive")]
pub use crabcheck_macros::test;

/// Everything a test module usually needs.
pub mod prelude {
    pub use crate::{
        Arbitrary,
        BoxGen,
        Config,
        Gen,
        GenExt,
        Mutate,
        Report,
        Shrink,
        Source,
        TestResult,
        any,
        assume,
        check,
        check_generator,
        check_mutator,
        check_par,
        check_shrinker,
        classify,
        gens,
        label,
        quickcheck,
        target,
    };
}
