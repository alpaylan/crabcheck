//! Run the property and print the report, suspects included.

fn main() {
    let report = locate_demo::check_addition();
    println!("{report}");
    if report.locate.as_ref().is_none_or(|l| l.suspects.is_empty()) {
        std::process::exit(1);
    }
}
