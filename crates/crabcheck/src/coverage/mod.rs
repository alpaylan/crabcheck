//! In-process coverage: read the LLVM profile counters of an instrumented
//! binary without writing profile files.
//!
//! Build the test binary with `-C instrument-coverage` (for example
//! `RUSTFLAGS="-C instrument-coverage" cargo test`, or `[build] rustflags`
//! in `.cargo/config.toml`) and enable the `coverage` feature. The build
//! script detects the flag and links the profile runtime; without it every
//! function here reports [`available`] as `false` and does nothing, so a
//! test suite that opts into coverage still compiles and runs uninstrumented.
//!
//! Counters are process-global and incremented atomically by the compiled
//! code, so a [`Snapshot`] taken with atomic loads is race-free, but other
//! threads running the same code pollute the [`Delta`] between two
//! snapshots. Coverage-using phases take the process-wide [`lock`] so that
//! crabcheck's own runs serialise; run the whole suite with
//! `RUST_TEST_THREADS=1` for exact attribution.

mod counters;
#[cfg_attr(not(crabcheck_instrumented), allow(dead_code))]
mod names;
pub(crate) mod tracker;

pub use {
    counters::{
        Delta,
        Function,
        Functions,
        Snapshot,
        available,
        hits_by_function,
        lock,
        profraw_image,
        snapshot,
        snapshot_range,
    },
    tracker::Summary,
};
