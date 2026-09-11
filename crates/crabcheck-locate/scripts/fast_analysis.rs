//! Thin CLI wrapper over [`crabcheck_locate::profiling::analyze`]. Emits the same
//! `--print-json` shape as before so the Python aggregators consume it
//! unchanged.
//!
//! Usage:
//!   crabcheck-profiling-fast-analyze <coverage_data_path> <module> <binary_path> [--print-json]

#[cfg(feature = "profiling")]
use {
    crabcheck_locate::profiling::analyze::{
        self,
        AnalyzeOptions,
    },
    std::{
        env,
        path::PathBuf,
    },
};

#[cfg(feature = "profiling")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 4 {
        eprintln!(
            "Usage: {} <coverage_data_path> <module> <binary_path> [--print-json]",
            args.first().map(|s| s.as_str()).unwrap_or("fast-analyze")
        );
        std::process::exit(1);
    }
    let coverage_dir = PathBuf::from(&args[1]);
    let module = args[2].clone();
    let binary_path = PathBuf::from(&args[3]);
    let print_json = args.iter().any(|a| a == "--print-json");

    let opts = AnalyzeOptions {
        coverage_dir: &coverage_dir,
        profdata_dir: None,
        binary_path: &binary_path,
        module: &module,
        // Loose filter for byte-identical output to the legacy CLI used by
        // the etna-ify benchmark's Python aggregators.
        strict_module_filter: false,
    };

    // The CLI ignores the diagnostics — they're a feature of the library
    // entry point (`quickcheck_with_locate`) and would clutter the JSON
    // output the etna-ify Python aggregators consume.
    let (regions, _diag) = analyze::analyze_coverage(&opts)?;
    let (positive_samples, negative_samples) = analyze::read_sample_counts(&coverage_dir)?;

    if print_json {
        // Match the legacy emit shape *exactly* — same keys, same order
        // (serde_json::json! sorts object keys alphabetically by default).
        // The Python aggregators (`scripts/aggregate_ranks_v2.py` etc.)
        // depend on this output being stable.
        let regions_json: Vec<serde_json::Value> = regions
            .iter()
            .map(|r| {
                serde_json::json!({
                    "file": r.file,
                    "function": r.function,
                    "start_line": r.start_line,
                    "start_col": r.start_col,
                    "end_line": r.end_line,
                    "end_col": r.end_col,
                    "positive_avg": r.positive_avg,
                    "negative_avg": r.negative_avg,
                    "delta": r.delta,
                    "ef": r.ef,
                    "ep": r.ep,
                    "nf": r.nf,
                    "np": r.np,
                    "suspiciousness": {
                        "tarantula": r.suspiciousness.tarantula,
                        "ochiai": r.suspiciousness.ochiai,
                        "dstar": r.suspiciousness.dstar,
                        "jaccard": r.suspiciousness.jaccard,
                        "op2": r.suspiciousness.op2,
                    }
                })
            })
            .collect();
        let out = serde_json::json!({
            "positive_samples": positive_samples,
            "negative_samples": negative_samples,
            "regions": regions_json,
        });
        println!("{}", serde_json::to_string(&out)?);
    } else {
        println!("{:60} {:>8} {:>8} {:>8} {:>8}", "File:Line", "Pos", "Neg", "Δ", "Ochiai");
        println!("{}", "-".repeat(93));
        for r in &regions {
            if r.delta <= 0.0 {
                continue;
            }
            let fname = std::path::Path::new(&r.file)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(r.file.as_str());
            let label = format!(
                "({}){}:{}:{} -> {}:{}",
                r.function, fname, r.start_line, r.start_col, r.end_line, r.end_col
            );
            println!(
                "{:60} {:>8.2} {:>8.2} {:+>8.2} {:>8.4}",
                label, r.positive_avg, r.negative_avg, r.delta, r.suspiciousness.ochiai
            );
        }
    }

    Ok(())
}

#[cfg(not(feature = "profiling"))]
fn main() {
    eprintln!("This binary requires the 'profiling' feature to be enabled.");
    std::process::exit(1);
}
