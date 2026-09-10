//! Integration test demonstrating `quickcheck_with_locate!` from `cargo test`.
//! No env-var setup, no `RUST_TEST_THREADS=1` — the orchestrator handles
//! per-call workdirs so concurrent tests don't race on `coverage/`.

// NOTE: integration tests (under `tests/`) are compiled as their own crate
// named after the file stem (e.g. `locate`), not the package crate. So we
// pass the explicit module name `"locate_demo"` to the 2-arg form of the
// macro — `env!("CARGO_CRATE_NAME")` would resolve to `"locate"` here and
// filter the actual library code out of the suspect list.

#[test]
fn finds_planted_overflow_bug() {
    let report = crabcheck_locate::quickcheck_with_locate!(
        locate_demo::property_addition_total,
        "locate_demo"
    );

    // Print the report to stderr so `cargo test` shows it without `--nocapture`.
    eprintln!("{report}");

    report.assert_failed();
    let top = report.top().expect("expected at least one suspect");

    assert!(
        top.region.file.ends_with("src/lib.rs"),
        "top suspect should be in src/lib.rs; got {}",
        top.region.file
    );

    // The buggy `if x > 100 { panic!(...) }` is on lines ~13-15 of src/lib.rs.
    // The function body containing the panic spans more than that, so just
    // require the suspect range to overlap the buggy block.
    let overlaps_panic_site = top.region.start_line <= 16 && top.region.end_line >= 12;
    assert!(
        overlaps_panic_site,
        "top suspect range {}-{} should overlap the buggy panic site near lines 13-15",
        top.region.start_line, top.region.end_line
    );

    // Crash-stack prior should have fired.
    assert!(
        top.panic_overlap,
        "expected the top suspect to be a panic-overlap region (crash-stack prior); panics={}",
        report.n_panics
    );
}

#[test]
fn parallel_safety_smoke() {
    // A second test in the same file. `cargo test` runs these in parallel by
    // default; the orchestrator's process-global mutex ensures concurrent
    // calls queue cleanly so neither run gets bad data.
    let report = crabcheck_locate::quickcheck_with_locate!(
        locate_demo::property_addition_total,
        "locate_demo"
    );
    eprintln!("[parallel_safety_smoke] {report}");
    report.assert_failed();
    assert!(
        report.top().is_some(),
        "expected suspects from parallel run too (workdir={})",
        report.workdir.display()
    );
}
