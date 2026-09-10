//! Checkers for generators, shrinkers and mutators.
//!
//! A shrinker is correct when every candidate stays inside the generator's
//! support and is strictly smaller under a size measure; a mutator is
//! correct when every mutant stays inside the support. Both are ordinary
//! properties, so these checkers run them through [`check`] and report a
//! counterexample (the value and the offending candidate) like any other
//! failure. Give the *support* of the generator, not just the invariant the
//! property cares about: a shrinker verified against a weaker predicate can
//! shrink into inputs the generator never produces.
//!
//! ```
//! use crabcheck::{check_shrinker, gens, Config};
//!
//! let die = gens::int(1u8..=6);
//! check_shrinker(&Config::default().tests(200), die, |d| (1..=6).contains(d), |d| u64::from(*d)).assert_passed();
//! ```

use std::{
    cell::Cell,
    collections::HashSet,
    fmt::Debug,
};

use crate::{
    Config,
    Gen,
    Report,
    Source,
    TestResult,
    check,
    classify,
    gens::BoxGen,
    label,
    runner::mix,
};

/// The configuration a checker runs with: the coverage filter defaults to
/// the crate that authored `support`, not to crabcheck's own closures.
fn authored_by<S>(config: &Config, support: &S) -> Config {
    #[cfg(feature = "coverage")]
    {
        let mut config = config.clone();
        if config.coverage.filter.is_empty() {
            config.coverage.filter =
                crate::runner::crate_of(std::any::type_name_of_val(support)).to_string();
        }
        config
    }
    #[cfg(not(feature = "coverage"))]
    {
        let _ = support;
        config.clone()
    }
}

/// Every generated value satisfies `support`.
pub fn check_generator<G>(
    config: &Config,
    generator: G,
    support: impl Fn(&G::Value) -> bool,
) -> Report
where
    G: Gen,
    G::Value: Clone + Debug,
{
    let config = authored_by(config, &support);
    check(&config, generator, move |value| {
        TestResult::check(support(&value), "generated value outside the support")
    })
}

/// Every shrink candidate of every generated value satisfies `support` and
/// is strictly smaller under `size`, and following first candidates from
/// the value reaches a fixpoint without revisiting a value.
pub fn check_shrinker<G>(
    config: &Config,
    generator: G,
    support: impl Fn(&G::Value) -> bool,
    size: impl Fn(&G::Value) -> u64,
) -> Report
where
    G: Gen + 'static,
    G::Value: Clone + Debug + 'static,
{
    let shared = BoxGen::new(generator);
    let hooks = shared.clone();
    let config = authored_by(config, &support);
    check(&config, shared, move |value| {
        let value_size = size(&value);
        let candidates = hooks.shrink(&value);
        classify(candidates.is_empty(), "no shrink candidates");
        for candidate in &candidates {
            if !support(candidate) {
                return TestResult::fail(format!(
                    "shrink candidate outside the support: {candidate:?}"
                ));
            }
            let candidate_size = size(candidate);
            if candidate_size >= value_size {
                return TestResult::fail(format!(
                    "shrink candidate not smaller: size {candidate_size} >= {value_size}: {candidate:?}"
                ));
            }
        }
        let mut current = value;
        let mut seen: HashSet<String> = HashSet::new();
        seen.insert(format!("{current:?}"));
        for _ in 0..1_000 {
            let Some(next) = hooks.shrink(&current).into_iter().next() else {
                return TestResult::Pass;
            };
            if !seen.insert(format!("{next:?}")) {
                return TestResult::fail(format!("shrink chain revisits {next:?}"));
            }
            current = next;
        }
        TestResult::fail(format!(
            "shrink chain did not reach a fixpoint within 1000 steps; last value {current:?}"
        ))
    })
}

/// Every mutant of every generated value satisfies `support`. Mutation
/// randomness derives from [`Config::seed`] so the run is reproducible. A
/// health warning notes a mutator that returns `None` or an identical value
/// for most inputs.
pub fn check_mutator<G>(
    config: &Config,
    generator: G,
    support: impl Fn(&G::Value) -> bool,
) -> Report
where
    G: Gen + 'static,
    G::Value: Clone + Debug + 'static,
{
    let shared = BoxGen::new(generator);
    let hooks = shared.clone();
    let base = config.seed.unwrap_or(0);
    let max_size = config.max_size;
    let counter = Cell::new(0u64);
    let config = authored_by(config, &support);
    let mut report = check(&config, shared, move |value| {
        let index = counter.get();
        counter.set(index + 1);
        let mut source = Source::new(mix(base, index), max_size);
        match hooks.mutate(&value, &mut source) {
            None => {
                label("no mutation");
                TestResult::Pass
            },
            Some(mutant) => {
                if !support(&mutant) {
                    return TestResult::fail(format!(
                        "mutant outside the support: {mutant:?} (from {value:?})"
                    ));
                }
                classify(
                    format!("{mutant:?}") == format!("{value:?}"),
                    "mutant identical to parent",
                );
                TestResult::Pass
            },
        }
    });
    let inert = report.labels.get("no mutation").copied().unwrap_or(0)
        + report.labels.get("mutant identical to parent").copied().unwrap_or(0);
    if report.tests >= 20 && inert * 2 > report.tests {
        report.warn(
            "mutator_inert",
            format!(
                "{inert} of {} inputs produced no mutation or an identical mutant",
                report.tests
            ),
        );
    }
    report
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{
            GenExt,
            gens,
        },
    };

    fn cfg() -> Config {
        Config::default().seed(1).tests(200).no_regressions()
    }

    #[test]
    fn generator_checker() {
        assert!(check_generator(&cfg(), gens::int(1u8..=6), |d| (1..=6).contains(d)).passed());
        let report = check_generator(&cfg(), gens::int(1u8..=6), |d| (1..=5).contains(d));
        assert!(!report.passed());
        assert_eq!(report.counterexample().unwrap().debug, "6");
    }

    #[test]
    fn shrinker_checker_accepts_built_ins_and_rejects_bad_hooks() {
        let die = gens::int(1u8..=6);
        assert!(check_shrinker(&cfg(), die, |d| (1..=6).contains(d), |d| u64::from(*d)).passed());
        let lists = gens::vec(gens::int(0i32..=9)).len(1..=5);
        let support =
            |v: &Vec<i32>| (1..=5).contains(&v.len()) && v.iter().all(|x| (0..=9).contains(x));
        let size = |v: &Vec<i32>| v.iter().map(|x| *x as u64 + 1).sum::<u64>();
        assert!(check_shrinker(&cfg(), lists, support, size).passed());

        let escapes = gens::int(1u8..=6).with_shrink(|_| vec![0u8]);
        let report = check_shrinker(&cfg(), escapes, |d| (1..=6).contains(d), |d| u64::from(*d));
        assert!(
            report
                .failure
                .as_ref()
                .unwrap()
                .message
                .as_deref()
                .unwrap()
                .contains("outside the support")
        );

        let grows = gens::int(1u8..=6).with_shrink(|d| vec![d.saturating_add(1).min(6)]);
        let report = check_shrinker(&cfg(), grows, |d| (1..=6).contains(d), |d| u64::from(*d));
        assert!(
            report.failure.as_ref().unwrap().message.as_deref().unwrap().contains("not smaller")
        );
    }

    #[test]
    fn mutator_checker() {
        assert!(check_mutator(&cfg(), gens::int(1u8..=6), |d| (1..=6).contains(d)).passed());
        let escapes = gens::int(1u8..=6).with_mutate(|d, _| Some(d + 100));
        let report = check_mutator(&cfg(), escapes, |d| (1..=6).contains(d));
        assert!(
            report
                .failure
                .as_ref()
                .unwrap()
                .message
                .as_deref()
                .unwrap()
                .contains("outside the support")
        );
        let inert = gens::int(1u8..=6).with_mutate(|d, _| Some(*d));
        let report = check_mutator(&cfg(), inert, |d| (1..=6).contains(d));
        assert!(report.passed());
        assert!(report.health.iter().any(|w| w.kind == "mutator_inert"), "{:?}", report.health);
    }
}
