pub mod fuzzing;
pub mod parallelism;
#[cfg(feature = "profiling")]
pub mod profiling;
pub mod quickcheck;
pub mod sbfl;
pub mod seedpool;
pub mod targeting;
#[cfg(feature = "tracing")]
pub mod tracing;
pub mod utils;

/// Run a profiling property test and locate the bug. The 1-arg form
/// auto-fills the `module` argument from `env!("CARGO_CRATE_NAME")` of the
/// *consuming* crate; the 2-arg form takes an explicit module name (use this
/// from integration tests under `tests/`, where `CARGO_CRATE_NAME` resolves
/// to the test file's stem rather than the library being tested).
///
/// 1-arg (use from `main` or `lib` unit tests):
/// ```ignore
/// crabcheck_locate::quickcheck_with_locate!(my_crate::property_foo);
/// ```
///
/// 2-arg (use from `tests/foo.rs` integration tests):
/// ```ignore
/// crabcheck_locate::quickcheck_with_locate!(my_crate::property_foo, "my_crate");
/// ```
///
/// Requires the `profiling` feature on the crabcheck dependency, plus the
/// usual coverage instrumentation flags on the consuming binary
/// (`-C instrument-coverage`, etc.). See `crabcheck/examples/locate-demo/`.
#[cfg(feature = "profiling")]
#[macro_export]
macro_rules! quickcheck_with_locate {
    ($f:expr) => {
        $crate::profiling::quickcheck_with_locate($f, env!("CARGO_CRATE_NAME"))
    };
    ($f:expr, $module:expr) => {
        $crate::profiling::quickcheck_with_locate($f, $module)
    };
}
