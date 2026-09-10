//! Interactive form: run the property and print the `LocateResult` to stdout.
//! Use this to see what the user-facing API looks like end-to-end.

fn main() {
    let report = crabcheck_locate::quickcheck_with_locate!(locate_demo::property_addition_total);
    println!("{report}");
    println!("\n(coverage data left in {:?} for inspection)", report.workdir);
}
