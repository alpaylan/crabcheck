//! The planted bug is the top suspect, from an integration test.

use crabcheck::{
    Status,
    coverage::Confidence,
};

#[test]
fn the_planted_bug_is_the_top_suspect() {
    let report = locate_demo::check_addition();
    eprintln!("{report}");
    assert_eq!(report.status, Status::Failed);
    let failure = report.failure.as_ref().expect("a failure");
    let panic = failure.location.as_ref().expect("a panic site");
    assert!(panic.file.ends_with("src/lib.rs"), "{}", panic.file);

    let locate = report.locate.as_ref().expect("localization ran");
    assert!(locate.passing > 0 && locate.failing > 0, "{locate:?}");
    let top = &locate.suspects[0];
    assert!(top.file.ends_with("src/lib.rs"), "{top:?}");
    assert!(top.function.contains("buggy_addition"), "{top:?}");
    assert!(top.start_line <= panic.line && panic.line <= top.end_line, "{top:?}");
    assert!(top.panic_overlap, "{top:?}");
    assert_eq!(top.confidence, Confidence::High, "{top:?}");
    assert!(top.scores.ochiai > 0.0, "{top:?}");
}
