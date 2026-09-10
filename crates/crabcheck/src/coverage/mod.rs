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
//!
//! # Profile files
//!
//! Every instrumented process writes `default_<hash>_<pid>.profraw` into
//! its working directory at exit, and with `-C instrument-coverage` in
//! `RUSTFLAGS` or `[build] rustflags` that includes build scripts and
//! proc-macro libraries, so each `rustc` invocation leaves a file too.
//! Nothing here needs those files. Send them to the target directory with
//! an `[env]` entry next to the flags in `.cargo/config.toml`:
//!
//! ```text
//! [build]
//! rustflags = ["-C", "instrument-coverage"]
//!
//! [env]
//! LLVM_PROFILE_FILE = { value = "target/profraw/%m_%p.profraw", relative = true }
//! ```
//!
//! `cargo llvm-cov` sets the variable itself. To get one profile per
//! property instead, set `Config::coverage.profraw_dir`; to stop the
//! process's own dump from a test, call [`suppress_default_dump`].
//!
//! # Regions and branches
//!
//! Stable Rust instruments *code regions*: every `if` and `match` arm,
//! every closure body and every short-circuit operand is a region with its
//! own counter, so the coverage report, the novelty signal of guided search
//! and the fault-localization spectrum already work at arm granularity.
//! Nightly adds true *branch regions* with a count for each outcome of a
//! condition:
//!
//! ```text
//! RUSTFLAGS="-C instrument-coverage -Z coverage-options=branch" cargo +nightly test
//! ```
//!
//! The mapping reader understands branch regions (and MC/DC regions, which
//! it skips), and the spectrum then scores the true and the false arm of
//! every condition separately; nothing else changes. Use it when a bug hides
//! in a condition that is evaluated on every run but decided differently.

mod counters;
pub(crate) mod locate;
pub(crate) mod mapping;
#[cfg_attr(not(crabcheck_instrumented), allow(dead_code))]
mod names;
pub(crate) mod tracker;

pub(crate) use locate::display_path;

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
        suppress_default_dump,
    },
    locate::{
        Confidence,
        Locate,
        Scores,
        Suspect,
    },
    tracker::Summary,
};
