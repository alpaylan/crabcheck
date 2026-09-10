//! What a property may return.

use std::fmt::Debug;

/// The result of one property invocation, as the runner sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The property held.
    Pass,
    /// The property returned `false`.
    False,
    /// The property failed with a message.
    Fail(String),
    /// The input was not applicable; try another one.
    Discard(String),
}

/// An explicit result a property can return when `bool` is not expressive
/// enough. See also the [`assume!`](crate::assume) macro.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TestResult {
    /// The property held.
    Pass,
    /// The property failed; the message goes in the report.
    Fail(String),
    /// The input was not applicable; it is discarded and another is tried.
    Discard(String),
}

impl TestResult {
    /// `Fail` with a message.
    pub fn fail(message: impl Into<String>) -> Self {
        TestResult::Fail(message.into())
    }

    /// `Discard` with a reason.
    pub fn discard(reason: impl Into<String>) -> Self {
        TestResult::Discard(reason.into())
    }

    /// `Pass` if `condition` holds, else `Fail` with `message`.
    pub fn check(condition: bool, message: impl Into<String>) -> Self {
        if condition { TestResult::Pass } else { TestResult::Fail(message.into()) }
    }
}

/// Types a property may return: `bool`, `()`, `Result<T, E: Debug>`,
/// [`TestResult`] and [`Outcome`]. A panic inside the property is a failure
/// as well.
pub trait Testable {
    /// Convert into the runner's view of the result.
    fn into_outcome(self) -> Outcome;
}

impl Testable for bool {
    fn into_outcome(self) -> Outcome {
        if self { Outcome::Pass } else { Outcome::False }
    }
}

impl Testable for () {
    fn into_outcome(self) -> Outcome {
        Outcome::Pass
    }
}

impl<T, E: Debug> Testable for Result<T, E> {
    fn into_outcome(self) -> Outcome {
        match self {
            Ok(_) => Outcome::Pass,
            Err(e) => Outcome::Fail(format!("{e:?}")),
        }
    }
}

impl Testable for TestResult {
    fn into_outcome(self) -> Outcome {
        match self {
            TestResult::Pass => Outcome::Pass,
            TestResult::Fail(m) => Outcome::Fail(m),
            TestResult::Discard(r) => Outcome::Discard(r),
        }
    }
}

impl Testable for Outcome {
    fn into_outcome(self) -> Outcome {
        self
    }
}

/// Discard the current input unless `condition` holds. The enclosing
/// property must return [`TestResult`].
///
/// ```
/// use crabcheck::{assume, gens, check, Config, TestResult};
///
/// let report = check(&Config::default().tests(20), gens::int(0..=100), |x: i32| {
///     assume!(x % 2 == 0);
///     TestResult::check(x % 2 == 0, "x is even")
/// });
/// assert!(report.passed());
/// assert!(report.discarded > 0);
/// ```
#[macro_export]
macro_rules! assume {
    ($condition:expr) => {
        if !$condition {
            return $crate::TestResult::Discard(
                concat!("assume: ", stringify!($condition)).to_string(),
            );
        }
    };
}
