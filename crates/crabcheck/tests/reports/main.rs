//! Snapshot tests of what crabcheck reports. Every scenario runs with a fixed
//! seed and an explicit config, and the rendered report (and its JSON) is
//! compared with the snapshot under `snapshots/`. Review a change with
//! `cargo insta review` (`cargo install cargo-insta`); CI fails on any drift.
//!
//! The report text is the interface an agent reads, so a diff here is a
//! change to that interface and deserves a look.

// The corpus is defined for the default feature set.
#![cfg(all(feature = "derive", feature = "json"))]

#[cfg(all(crabcheck_instrumented, feature = "coverage"))]
mod planted;

use std::path::Path;

use crabcheck::{
    Arbitrary,
    Config,
    GenExt,
    Report,
    TestResult,
    any,
    check,
    check_shrinker,
    classify,
    cover,
    gens,
    label,
    target,
};

/// `check` reads `CRABCHECK_*` variables, which would silently change every
/// snapshot; refuse to run with any of them set.
fn no_env_overrides() {
    let set: Vec<String> =
        std::env::vars().map(|(k, _)| k).filter(|k| k.starts_with("CRABCHECK_")).collect();
    assert!(set.is_empty(), "unset {} before running the snapshot tests", set.join(", "));
}

fn base(name: &str) -> Config {
    no_env_overrides();
    let mut config = Config::default().tests(200).seed(0x5eed).name(name);
    config.coverage.enabled = false;
    config.regressions.save = false;
    config.regressions.replay = false;
    config
}

/// Redact what legitimately varies between runs: timings, the panic site's
/// position in this file, timestamps and temporary paths. The
/// `slow_generation` warning compares timings, so it is dropped entirely.
fn settings(paths: &[&Path]) -> insta::Settings {
    let mut settings = insta::Settings::clone_current();
    settings.add_filter(r"\b\d+(\.\d+)? (µs|ms|s)\b", "[time]");
    settings.add_filter(r"    slowest input: .*", "    slowest input: [input]");
    settings.add_filter(r#""slowest": "(?:[^"\\]|\\.)*""#, r#""slowest": "[input]""#);
    settings.add_filter(r"\n  warning \[slow_generation\][^\n]*", "");
    settings.add_filter(r"reports/(main|planted)\.rs:\d+:\d+", "reports/$1.rs:[L:C]");
    settings.add_filter(r#""line": \d+,(\s*)"column": \d+"#, r#""line": "[L]",$1"column": "[C]""#);
    settings.add_filter(r#"\{\s*"secs": \d+,\s*"nanos": \d+\s*\}"#, r#""[duration]""#);
    settings.add_filter(r#""created": \d+"#, r#""created": "[time]""#);
    for path in paths {
        settings.add_filter(&regex_escape(&path.display().to_string()), "[dir]");
    }
    settings
}

fn regex_escape(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if r"\.+*?()|[]{}^$#".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn snapshot(name: &str, report: &Report, paths: &[&Path]) {
    let mut report = report.clone();
    report.health.retain(|w| w.kind != "slow_generation");
    settings(paths).bind(|| {
        insta::assert_snapshot!(format!("{name}_text"), report.to_string());
        #[cfg(feature = "json")]
        insta::assert_snapshot!(
            format!("{name}_json"),
            serde_json::to_string_pretty(&report).expect("report serializes")
        );
    });
}

fn is_sorted(v: &[i32]) -> bool {
    v.windows(2).all(|w| w[0] <= w[1])
}

fn buggy_insertion_sort(array: &mut [i32]) {
    for i in 1..array.len().saturating_sub(1) {
        let mut j = i;
        while j > 0 && array[j - 1] > array[j] {
            array.swap(j - 1, j);
            j -= 1;
        }
    }
}

#[test]
fn passing_with_labels() {
    let report = check(&base("sort_keeps_length"), gens::vec(gens::int(-50i32..=50)), |mut v| {
        classify(v.is_empty(), "empty");
        label(if v.len() < 8 { "short" } else { "long" });
        let before = v.len();
        v.sort();
        v.len() == before && is_sorted(&v)
    });
    snapshot("passing_with_labels", &report, &[]);
}

#[test]
fn false_property_shrinks() {
    let report = check(&base("short_vectors"), gens::vec(gens::int(0u8..=9)), |v| v.len() < 3);
    snapshot("false_property_shrinks", &report, &[]);
}

#[test]
fn panicking_property() {
    let report = check(&base("small_sums"), gens::vec(gens::int(0i64..=40)), |v| {
        if v.iter().sum::<i64>() > 100 {
            panic!("sum too large: {}", v.iter().sum::<i64>());
        }
    });
    snapshot("panicking_property", &report, &[]);
}

#[test]
fn error_result() {
    let report = check(&base("not_divisible_by_seven"), gens::int(1i32..=1000), |x| {
        if x % 7 == 0 { Err(format!("{x} is divisible by seven")) } else { Ok(()) }
    });
    snapshot("error_result", &report, &[]);
}

#[test]
fn discards_exhaust_the_ratio() {
    let report = check(&base("only_zero"), gens::int(0i32..=100), |x| {
        if x != 0 { TestResult::discard("not zero") } else { TestResult::check(true, "") }
    });
    snapshot("discards_exhaust_the_ratio", &report, &[]);
}

#[test]
fn insertion_sort_bug_shrinks_to_two_elements() {
    let report =
        check(&base("buggy_insertion_sort"), gens::vec(gens::int(-100i32..=100)), |mut v| {
            buggy_insertion_sort(&mut v);
            is_sorted(&v)
        });
    snapshot("insertion_sort_bug", &report, &[]);
}

#[test]
fn replay_reproduces_a_point() {
    let first = check(&base("short_vectors"), gens::vec(gens::int(0u8..=9)), |v| v.len() < 3);
    let point = first.failure.as_ref().and_then(|f| f.replay).expect("a random-phase failure");
    let mut config = base("short_vectors");
    config.replay = Some(point);
    let report = check(&config, gens::vec(gens::int(0u8..=9)), |v| v.len() < 3);
    snapshot("replay_reproduces_a_point", &report, &[]);
}

#[test]
fn stale_regression_entry_is_reported() {
    let dir = std::env::temp_dir().join(format!("crabcheck-snapshots-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let entry = crabcheck::regressions::Entry {
        name: "stale".to_string(),
        seed: 1,
        iteration: 0,
        debug: "[999]".to_string(),
        shrunk: None,
        created: 0,
    };
    crabcheck::regressions::save(&dir, &entry).expect("write the entry");
    let mut config = base("stale");
    config.regressions.dir = dir.clone();
    config.regressions.replay = true;
    let report = check(&config, gens::vec(gens::int(0i32..=9)), |_| true);
    snapshot("stale_regression_entry", &report, &[&dir]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn targeted_search_reports_its_best() {
    let mut config = base("heavy_vectors");
    config.target.tests = 100;
    let report = check(&config, gens::vec(gens::int(0u8..=9)).len(4..=4), |v: Vec<u8>| {
        target(v.iter().map(|&x| f64::from(x)).sum());
        true
    });
    snapshot("targeted_search", &report, &[]);
}

#[test]
fn a_bad_shrinker_is_rejected() {
    let generator = gens::int(0i32..=100).with_shrink(|&x| vec![x + 1]);
    let report = check_shrinker(
        &base("grows_out_of_range"),
        generator,
        |x| (0..=100).contains(x),
        |x| *x as u64,
    );
    snapshot("bad_shrinker_rejected", &report, &[]);
}

#[derive(Clone, Debug, Arbitrary)]
struct Point {
    x: i8,
    y: i8,
}

#[test]
fn derived_type_shrinks_field_by_field() {
    let report = check(&base("x_at_most_y"), any::<Point>(), |p| p.x <= p.y);
    snapshot("derived_type_shrinks", &report, &[]);
}

#[test]
fn dependent_pair_shrinks_within_support() {
    let indexed =
        gens::dependent(gens::vec(gens::int(0i32..=100)).len(1..=8), |v| gens::int(0..v.len()));
    let report = check(&base("small_at_index"), indexed, |(v, i)| v[i] < 50);
    snapshot("dependent_pair_shrinks", &report, &[]);
}

#[test]
fn cover_shares_are_reported() {
    let report = check(&base("covered"), gens::vec(gens::int(0i32..=9)), |v| {
        cover(v.len() > 100, "long");
        cover(true, "always");
        true
    });
    snapshot("cover_shares", &report, &[]);
}

/// The examples double as an ecosystem check: run each with a fixed seed and
/// snapshot what it prints. They are built by `cargo test` (not by
/// `cargo test --test reports`, which skips this with a note, and which can
/// also see stale example binaries after a library change).
#[test]
fn examples_print_the_same_reports() {
    no_env_overrides();
    let exe = std::env::current_exe().expect("test binary path");
    let examples =
        exe.parent().and_then(Path::parent).expect("target profile dir").join("examples");
    let workdir = std::env::temp_dir().join(format!("crabcheck-examples-{}", std::process::id()));
    std::fs::create_dir_all(&workdir).expect("workdir");
    for name in ["bst", "insertion_sort", "sort_performance"] {
        let binary = examples.join(name);
        if !binary.exists() {
            eprintln!(
                "skipping {name}: {} not built (run `cargo test` without `--test`)",
                binary.display()
            );
            continue;
        }
        let output = std::process::Command::new(&binary)
            .current_dir(&workdir)
            .env("CRABCHECK_SEED", "7")
            .env("CRABCHECK_REGRESSIONS", "0")
            .env("CRABCHECK_COVERAGE", "0")
            .output()
            .expect("run the example");
        let text = format!(
            "exit: {}\n--- stdout ---\n{}--- stderr ---\n{}",
            output.status.code().map_or("signal".to_string(), |c| c.to_string()),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        settings(&[&workdir]).bind(|| insta::assert_snapshot!(format!("example_{name}"), text));
    }
    let _ = std::fs::remove_dir_all(&workdir);
}

/// Instrumented builds with the `coverage` feature only: the coverage lines
/// and the suspect list. Needs `RUST_TEST_THREADS=1`, otherwise a warning
/// about other threads joins the report; the scenarios are skipped with a
/// note in that case.
#[cfg(all(crabcheck_instrumented, feature = "coverage"))]
mod instrumented {
    use super::*;

    fn ready() -> bool {
        if std::env::var("RUST_TEST_THREADS").as_deref() != Ok("1") {
            eprintln!("skipping instrumented snapshots: set RUST_TEST_THREADS=1");
            return false;
        }
        true
    }

    fn instrumented(name: &str) -> Config {
        let mut config = base(name);
        config.coverage.enabled = true;
        config.coverage.filter = "reports::planted".to_string();
        config
    }

    #[test]
    fn coverage_lines() {
        if !ready() {
            return;
        }
        let report = check(&instrumented("classify_small"), gens::int(-100i32..=100), |x| {
            !planted::classify(x).is_empty()
        });
        snapshot("instrumented_coverage_lines", &report, &[]);
    }

    #[test]
    fn suspects_name_the_planted_bug() {
        if !ready() {
            return;
        }
        let report = check(&instrumented("classify_all"), gens::int(-100i32..=2000), |x| {
            !planted::classify(x).is_empty()
        });
        snapshot("instrumented_suspects", &report, &[]);
    }
}
