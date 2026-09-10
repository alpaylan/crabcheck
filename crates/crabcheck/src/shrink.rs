//! The shrink driver: greedy descent over a generator's candidates.
//!
//! Starting from the failing input, ask the generator for candidates, take
//! the first one that still fails with the same signature and is not
//! rendered longer than the current best, and repeat until no candidate is
//! accepted or the budget runs out. A seen-set of `Debug` renderings stops
//! cycles; the step and time budget bound the wall clock.

use std::{
    collections::HashSet,
    fmt::Debug,
    hash::{
        BuildHasher,
        RandomState,
    },
    time::{
        Duration,
        Instant,
    },
};

use crate::{
    config::ShrinkConfig,
    report::{
        Location,
        ShrinkStats,
    },
};

/// What the oracle returns for a candidate that fails the same way: the
/// failure's message and location.
pub(crate) type SameFailure = Option<(Option<String>, Option<Location>)>;

/// Result of shrinking: the best value, the failure it produced (if a
/// candidate was accepted), and the statistics.
pub(crate) struct Shrunk<T> {
    pub value: T,
    pub failure: SameFailure,
    pub stats: ShrinkStats,
}

/// Shrink `initial`. `candidates` proposes simpler values; `fails` runs the
/// property and reports whether the candidate fails the same way; `render`
/// gives the `Debug` rendering and its untruncated length.
pub(crate) fn shrink<T: Clone + Debug>(
    config: &ShrinkConfig,
    initial: T,
    candidates: impl Fn(&T) -> Vec<T>,
    mut fails: impl FnMut(T) -> SameFailure,
    render: impl Fn(&T) -> (String, bool, usize),
) -> Shrunk<T> {
    let hasher = RandomState::new();
    let started = Instant::now();
    let deadline = config.timeout_ms.map(|ms| started + Duration::from_millis(ms));
    let mut stats = ShrinkStats::default();
    let exhausted = |stats: &ShrinkStats| {
        stats.steps >= config.max_steps || deadline.is_some_and(|d| Instant::now() >= d)
    };

    let mut current = initial;
    let (current_debug, _, mut current_len) = render(&current);
    let mut seen: HashSet<u64> = HashSet::new();
    seen.insert(hasher.hash_one(&current_debug));
    let mut failure: SameFailure = None;

    'outer: loop {
        if exhausted(&stats) {
            stats.budget_exhausted = true;
            break;
        }
        let batch = candidates(&current);
        if batch.is_empty() {
            break;
        }
        for candidate in batch {
            if exhausted(&stats) {
                stats.budget_exhausted = true;
                break 'outer;
            }
            let (debug, _, len) = render(&candidate);
            if !seen.insert(hasher.hash_one(&debug)) {
                stats.rejected_seen += 1;
                continue;
            }
            if len > current_len {
                stats.rejected_not_smaller += 1;
                continue;
            }
            stats.steps += 1;
            if let Some(details) = fails(candidate.clone()) {
                stats.accepted += 1;
                failure = Some(details);
                current = candidate;
                current_len = len;
                continue 'outer;
            }
        }
        break;
    }
    stats.duration = started.elapsed();
    Shrunk { value: current, failure, stats }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{
            Gen,
            gens,
        },
    };

    fn render(v: &i64) -> (String, bool, usize) {
        let s = format!("{v:?}");
        let len = s.len();
        (s, false, len)
    }

    #[test]
    fn descends_to_the_boundary() {
        let shrunk = shrink(
            &ShrinkConfig::default(),
            1000i64,
            |v| vec![v / 2, v - 1],
            |v| (v >= 300).then_some((Some(format!("v={v}")), None)),
            render,
        );
        assert_eq!(shrunk.value, 300);
        assert_eq!(shrunk.failure.unwrap().0.as_deref(), Some("v=300"));
        assert!(shrunk.stats.accepted > 0);
        assert!(!shrunk.stats.budget_exhausted);
    }

    #[test]
    fn growing_candidates_terminate() {
        let shrunk =
            shrink(&ShrinkConfig::default(), 5i64, |v| vec![v + 1], |_| Some((None, None)), render);
        // 6..9 are accepted (same length); 10 is longer and rejected.
        assert_eq!(shrunk.value, 9);
        assert_eq!(shrunk.stats.rejected_not_smaller, 1);
    }

    #[test]
    fn budget_and_seen_set() {
        let config = ShrinkConfig { max_steps: 2, ..ShrinkConfig::default() };
        let shrunk =
            shrink(&config, 100i64, |v| vec![v - 1, v - 1], |_| Some((None, None)), render);
        assert_eq!(shrunk.value, 98);
        assert!(shrunk.stats.budget_exhausted);
        let none =
            shrink(&ShrinkConfig::default(), 3i64, |_| Vec::new(), |_| Some((None, None)), render);
        assert_eq!(none.value, 3);
        assert!(none.failure.is_none());
    }

    #[test]
    fn built_in_generators_shrink_to_golden_values() {
        let generator = gens::vec(gens::int(0i32..=1000));
        let fails = |v: &Vec<i32>| v.iter().sum::<i32>() > 100;
        let shrunk = shrink(
            &ShrinkConfig::default(),
            vec![400, 3, 250, 99],
            |v| generator.shrink(v),
            |v| fails(&v).then_some((None, None)),
            |v| {
                let s = format!("{v:?}");
                let len = s.len();
                (s, false, len)
            },
        );
        assert_eq!(shrunk.value, vec![101]);

        let generator = gens::vec(gens::int(-1000i32..=1000));
        let unsorted = |v: &Vec<i32>| v.windows(2).any(|w| w[0] > w[1]);
        let shrunk = shrink(
            &ShrinkConfig::default(),
            vec![5, -3, 800, 2, 2],
            |v| generator.shrink(v),
            |v| unsorted(&v).then_some((None, None)),
            |v| {
                let s = format!("{v:?}");
                let len = s.len();
                (s, false, len)
            },
        );
        assert_eq!(shrunk.value.len(), 2, "{:?}", shrunk.value);
        assert!(shrunk.value.iter().all(|x| x.abs() <= 1), "{:?}", shrunk.value);
    }
}
