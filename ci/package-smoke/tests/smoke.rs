//! Exercises the derive, the test attribute, a failing check with replay,
//! a checker and coverage, all through the packaged crates.

use crabcheck::{
    Config,
    Status,
    any,
    check,
    check_shrinker,
    cover,
    gens,
};
use crabcheck_smoke::Interval;

fn config(name: &str, seed: u64) -> Config {
    let mut config = Config::default().tests(300).seed(seed).name(name);
    config.regressions.save = false;
    config.regressions.replay = false;
    config
}

#[crabcheck::test]
fn width_is_non_negative(i: Interval) -> bool {
    i.width() >= 0
}

#[test]
fn a_failure_shrinks_and_replays() {
    let mut config = config("small_sums", 1);
    let report = check(&config, gens::vec(gens::int(-100i32..=100)), |v| v.iter().sum::<i32>() < 50);
    assert_eq!(report.status, Status::Failed, "{report}");
    let failure = report.failure.as_ref().unwrap();
    assert_eq!(failure.shrunk.as_ref().unwrap().debug, "[50]", "{report}");
    config.replay = failure.replay;
    let again = check(&config, gens::vec(gens::int(-100i32..=100)), |v| v.iter().sum::<i32>() < 50);
    assert!(again.replay.as_ref().is_some_and(|r| r.reproduced), "{again}");
}

#[test]
fn the_derived_shrinker_is_valid() {
    let support = |i: &Interval| (0..=100).contains(&i.lo) && (0..=100).contains(&i.hi);
    check_shrinker(&config("interval_shrinker", 2), any::<Interval>(), support, |i| (i.lo + i.hi) as u64)
        .assert_passed();
}

#[test]
fn coverage_matches_the_build() {
    let mut config = config("width", 3);
    config.coverage.filter = "crabcheck_smoke".to_string();
    let report = check(&config, any::<Interval>(), |i| {
        cover(i.width() > 50, "wide");
        i.width() >= 0
    });
    report.assert_passed();
    assert!(report.covers.contains_key("wide"), "{report}");
    if crabcheck::coverage::available() {
        assert!(report.coverage.is_some(), "{report}");
        assert!(report.to_string().contains("coverage:"), "{report}");
    } else {
        assert!(report.coverage.is_none(), "{report}");
    }
}
