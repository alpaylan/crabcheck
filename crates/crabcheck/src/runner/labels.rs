//! Labels, classification and target scores: the property's side channel
//! to the runner.
//!
//! Call [`label`] or [`classify`] from inside a property to see how inputs
//! are distributed, and [`target`] to steer the search toward inputs that
//! maximise a score. Outside a running property all three are no-ops.

use std::cell::RefCell;

#[derive(Default)]
struct Context {
    labels: Vec<String>,
    scores: Vec<f64>,
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

/// What a property reported through the side channel.
pub(crate) struct Observed {
    pub labels: Vec<String>,
    /// The maximum of the scores passed to [`target`], if any.
    pub score: Option<f64>,
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
    (result, Observed { labels: context.labels, score })
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
        });
        assert_eq!(observed.labels, vec!["a", "b"]);
        assert_eq!(observed.score, Some(5.0));
        let ((), none) = collect(|| ());
        assert!(none.labels.is_empty());
        assert_eq!(none.score, None);
    }
}
