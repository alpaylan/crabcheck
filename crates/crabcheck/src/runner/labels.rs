//! Labels, classification and target scores: the property's side channel
//! to the runner.
//!
//! Call [`label`] or [`classify`] from inside a property to see how inputs
//! are distributed, [`feature`] to record a number or a category per input,
//! [`cover`] to require a share of inputs to meet a condition, and
//! [`target`] to steer the search toward inputs that maximise a score.
//! Outside a running property all of them are no-ops.

use std::cell::RefCell;

/// A value recorded with [`feature`]: a number, summarised in the report
/// with its quantiles and a histogram, or a piece of text, counted like a
/// label.
#[derive(Clone, Debug, PartialEq)]
pub enum FeatureValue {
    /// A number; non-finite values are ignored.
    Number(f64),
    /// A category.
    Text(String),
}

macro_rules! numeric_features {
    ($($t:ty),*) => {$(
        impl From<$t> for FeatureValue {
            fn from(value: $t) -> Self {
                FeatureValue::Number(value as f64)
            }
        }
    )*};
}
numeric_features!(f64, f32, i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize);

impl From<bool> for FeatureValue {
    fn from(value: bool) -> Self {
        FeatureValue::Text(value.to_string())
    }
}

impl From<char> for FeatureValue {
    fn from(value: char) -> Self {
        FeatureValue::Text(value.to_string())
    }
}

impl From<&str> for FeatureValue {
    fn from(value: &str) -> Self {
        FeatureValue::Text(value.to_string())
    }
}

impl From<String> for FeatureValue {
    fn from(value: String) -> Self {
        FeatureValue::Text(value)
    }
}

#[derive(Default)]
struct Context {
    labels: Vec<String>,
    scores: Vec<f64>,
    covers: Vec<(String, bool)>,
    features: Vec<(String, FeatureValue)>,
}

thread_local! {
    static CONTEXT: RefCell<Option<Context>> = const { RefCell::new(None) };
}

/// Tag the current test case with `name`. The report shows what share of
/// passing tests carried each label.
pub fn label(name: impl Into<String>) {
    CONTEXT.with(|context| {
        if let Some(context) = context.borrow_mut().as_mut() {
            context.labels.push(name.into());
        }
    });
}

/// Tag the current test case with `name` when `condition` holds.
pub fn classify(condition: bool, name: impl Into<String>) {
    if condition {
        label(name);
    }
}

/// Record a feature of the current input: a number (a length, a depth, a
/// sum) or a category (a variant name, a bucket). The report summarises
/// each numeric feature over the passing tests with its minimum, median,
/// 90th percentile, maximum and a histogram, and counts each text value
/// like a [`label`] named `feature=value`. Use it to see the shape of what
/// the generator produces without printing inputs:
///
/// ```
/// use crabcheck::{check, feature, gens, Config};
///
/// let report = check(&Config::default().tests(200), gens::vec(gens::int(0i32..=9)), |v| {
///     feature("len", v.len());
///     feature("parity", if v.len().is_multiple_of(2) { "even" } else { "odd" });
///     true
/// });
/// assert!(report.features["len"].max <= 100.0);
/// assert!(report.labels.contains_key("parity=even"));
/// ```
pub fn feature(name: impl Into<String>, value: impl Into<FeatureValue>) {
    let value = value.into();
    if let FeatureValue::Number(n) = value
        && !n.is_finite()
    {
        return;
    }
    CONTEXT.with(|context| {
        if let Some(context) = context.borrow_mut().as_mut() {
            context.features.push((name.into(), value));
        }
    });
}

/// Report a score for the current input. When a property calls this, the
/// runner follows the random phase with a targeted phase that mutates the
/// best-scoring inputs to push the score higher (see
/// [`Config::target`](crate::Config::target)); use it to steer toward
/// slow paths, deep recursion, large sums, or any measure that correlates
/// with the bug you are hunting. The highest score and the input that
/// reached it are in the report. Several calls in one test case count as
/// their maximum. Non-finite scores are ignored.
pub fn target(score: f64) {
    if !score.is_finite() {
        return;
    }
    CONTEXT.with(|context| {
        if let Some(context) = context.borrow_mut().as_mut() {
            context.scores.push(score);
        }
    });
}

/// Declare a case the inputs must exercise: like [`classify`], but the
/// runner reports the share of passing tests that met `condition` under
/// `label`, and warns when it falls below
/// [`Config::coverage.min_cover`](crate::config::CoverageConfig::min_cover).
pub fn cover(condition: bool, label: impl Into<String>) {
    CONTEXT.with(|context| {
        if let Some(context) = context.borrow_mut().as_mut() {
            context.covers.push((label.into(), condition));
        }
    });
}

/// What a property reported through the side channel.
pub(crate) struct Observed {
    pub labels: Vec<String>,
    /// The maximum of the scores passed to [`target`], if any.
    pub score: Option<f64>,
    /// Cover labels and whether their condition held.
    pub covers: Vec<(String, bool)>,
    /// Features recorded with [`feature`], in call order.
    pub features: Vec<(String, FeatureValue)>,
}

/// Run `f` with the side channel active and return what it reported.
pub(crate) fn collect<R>(f: impl FnOnce() -> R) -> (R, Observed) {
    CONTEXT.with(|context| *context.borrow_mut() = Some(Context::default()));
    let result = f();
    let context = CONTEXT.with(|context| context.borrow_mut().take()).unwrap_or_default();
    let score = context
        .scores
        .iter()
        .copied()
        .fold(None, |best: Option<f64>, s| Some(best.map_or(s, |b| b.max(s))));
    (
        result,
        Observed {
            labels: context.labels,
            score,
            covers: context.covers,
            features: context.features,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_channel_is_active_only_inside_collect() {
        label("ignored");
        target(1.0);
        let ((), observed) = collect(|| {
            label("a");
            classify(true, "b");
            classify(false, "c");
            target(2.0);
            target(f64::NAN);
            target(5.0);
            target(3.0);
            cover(true, "c");
            cover(false, "d");
            feature("len", 3usize);
            feature("kind", "leaf");
            feature("nan", f64::NAN);
        });
        assert_eq!(observed.labels, vec!["a", "b"]);
        assert_eq!(observed.score, Some(5.0));
        assert_eq!(observed.covers, vec![("c".to_string(), true), ("d".to_string(), false)]);
        assert_eq!(
            observed.features,
            vec![
                ("len".to_string(), FeatureValue::Number(3.0)),
                ("kind".to_string(), FeatureValue::Text("leaf".to_string()))
            ]
        );
        let ((), none) = collect(|| ());
        assert!(none.labels.is_empty());
        assert_eq!(none.score, None);
    }
}
