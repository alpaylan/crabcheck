//! Single-pass SBFL analysis that avoids writing per-snapshot JSON to
//! disk. Spawns `llvm-cov export` per snapshot, streams stdout into a
//! region-counter hashmap via serde_json, and emits the same JSON shape
//! as `crabcheck-profiling-analysis --print-json`.
//!
//! Assumes `llvm-profdata merge` has already produced the per-snapshot
//! .profdata files in `<coverage>/../profdata/`.
//!
//! Usage:
//!   crabcheck-profiling-fast-analyze <coverage_data_path> <module> <binary_path> [--print-json]

#[cfg(feature = "profiling")]
use {
    crabcheck::sbfl::Suspiciousness,
    rayon::prelude::*,
    rustc_demangle::demangle,
    serde::Deserialize,
    serde_json::Value,
    std::{
        collections::HashMap,
        env,
        fs::File,
        io::BufReader,
        path::{Path, PathBuf},
        process::{Command, Stdio},
        sync::Mutex,
    },
};

#[cfg(feature = "profiling")]
#[derive(Debug, Clone, Hash, Eq, PartialEq, PartialOrd, Ord)]
struct RegionKey {
    fname: String,
    func: String,
    sl: u32,
    sc: u32,
    el: u32,
    ec: u32,
}

#[cfg(feature = "profiling")]
#[derive(Deserialize)]
struct Indices {
    positives: Vec<usize>,
    negatives: Vec<usize>,
}

// Minimal deserialization of `llvm-cov export --format=text`:
//   { "data": [{ "functions": [{ "name": ..., "filenames": [...],
//                                 "regions": [[sl,sc,el,ec,count,...], ...] }] }] }
#[cfg(feature = "profiling")]
#[derive(Deserialize)]
struct LlvmCovExport {
    data: Vec<LlvmCovData>,
}

#[cfg(feature = "profiling")]
#[derive(Deserialize)]
struct LlvmCovData {
    functions: Vec<LlvmCovFunction>,
}

#[cfg(feature = "profiling")]
#[derive(Deserialize)]
struct LlvmCovFunction {
    name: String,
    filenames: Vec<String>,
    regions: Vec<Vec<Value>>,
}

#[cfg(feature = "profiling")]
#[derive(Default, Clone)]
struct Counts {
    ef: u64,        // failing snapshots where region was hit (count > 0)
    ep: u64,        // passing snapshots where region was hit
    neg_count: u64, // sum of counts over failing snapshots
    pos_count: u64, // sum of counts over passing snapshots
}

#[cfg(feature = "profiling")]
fn region_u64(region: &[Value], i: usize) -> u64 {
    region
        .get(i)
        .and_then(|v| v.as_u64().or_else(|| v.as_i64().map(|x| x.max(0) as u64)))
        .unwrap_or(0)
}

#[cfg(feature = "profiling")]
fn region_u32(region: &[Value], i: usize) -> u32 {
    region_u64(region, i) as u32
}

/// Return the absolute path to rustc's llvm-tools bin directory so we can
/// find `llvm-profdata` / `llvm-cov` without relying on the caller's PATH.
/// Matches what workloads' old `instrumentation.sh` did via
/// `rustc --print sysroot` + `rustc -vV | grep host`.
#[cfg(feature = "profiling")]
fn rustc_llvm_bin() -> Option<String> {
    let sysroot = Command::new("rustc").arg("--print").arg("sysroot").output().ok()?;
    if !sysroot.status.success() {
        return None;
    }
    let sysroot_path = String::from_utf8(sysroot.stdout).ok()?.trim().to_string();

    let vv = Command::new("rustc").arg("-vV").output().ok()?;
    if !vv.status.success() {
        return None;
    }
    let host = String::from_utf8(vv.stdout).ok()?
        .lines()
        .find_map(|line| line.strip_prefix("host: ").map(|s| s.trim().to_string()))?;

    Some(format!("{sysroot_path}/lib/rustlib/{host}/bin"))
}

#[cfg(feature = "profiling")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 4 {
        eprintln!(
            "Usage: {} <coverage_data_path> <module> <binary_path> [--print-json]",
            args.get(0).map(|s| s.as_str()).unwrap_or("fast-analyze")
        );
        std::process::exit(1);
    }
    let coverage_data_path = &args[1];
    let module = &args[2];
    let binary_path = &args[3];
    let print_json = args.iter().any(|a| a == "--print-json");

    // Augment PATH with rustc's llvm-tools so subprocesses find llvm-profdata / llvm-cov
    // without the caller having to prepend it. This matches what
    // instrumentation.sh used to do via `rustc --print sysroot`.
    let augmented_path = match rustc_llvm_bin() {
        Some(bin) => {
            let existing = env::var("PATH").unwrap_or_default();
            if existing.is_empty() {
                bin
            } else {
                format!("{bin}:{existing}")
            }
        },
        None => env::var("PATH").unwrap_or_default(),
    };

    // Read indices.json
    let indices_path = format!("{}/indices.json", coverage_data_path);
    let indices: Indices =
        serde_json::from_reader(BufReader::new(File::open(&indices_path)?))?;
    let positive_samples = indices.positives.len();
    let negative_samples = indices.negatives.len();

    let coverage_dir = PathBuf::from(coverage_data_path);

    // profdata lives next to coverage: `{parent}/profdata/snapshot_iteration_*.profdata`.
    // Create the directory if missing; we merge lazily per snapshot below.
    let profdata_dir: PathBuf = {
        let parent = coverage_dir.parent().unwrap_or_else(|| Path::new("."));
        let target = parent.join("profdata");
        if let Err(e) = std::fs::create_dir_all(&target) {
            return Err(format!("create {target:?}: {e}").into());
        }
        target
    };

    // Assemble snapshot list: (idx, is_negative)
    let mut snapshots: Vec<(usize, bool)> =
        Vec::with_capacity(positive_samples + negative_samples);
    snapshots.extend(indices.positives.iter().map(|i| (*i, false)));
    snapshots.extend(indices.negatives.iter().map(|i| (*i, true)));

    let accum: Mutex<HashMap<RegionKey, Counts>> = Mutex::new(HashMap::new());

    snapshots.par_iter().try_for_each(|(idx, is_neg)| -> Result<(), String> {
        let profdata_path =
            profdata_dir.join(format!("snapshot_iteration_{}.profdata", idx));

        // Lazily merge profraw → profdata if the profdata doesn't exist yet.
        if !profdata_path.exists() {
            let profraw_path =
                coverage_dir.join(format!("snapshot_iteration_{}.profraw", idx));
            if !profraw_path.exists() {
                return Ok(()); // neither exists — skip
            }
            let status = Command::new("llvm-profdata")
                .arg("merge")
                .arg("-sparse")
                .arg(&profraw_path)
                .arg("-o")
                .arg(&profdata_path)
                .env("PATH", &augmented_path)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map_err(|e| format!("spawn llvm-profdata (iter {idx}): {e}"))?;
            if !status.success() {
                return Err(format!("llvm-profdata merge failed for iter {idx}"));
            }
        }

        let child = Command::new("llvm-cov")
            .arg("export")
            .arg(binary_path)
            .arg("--instr-profile")
            .arg(&profdata_path)
            .arg("--format=text")
            .arg("--skip-expansions")
            .env("PATH", &augmented_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("spawn llvm-cov: {e}"))?;

        let stdout = child.stdout.ok_or_else(|| "no stdout".to_string())?;
        let export: LlvmCovExport =
            serde_json::from_reader(BufReader::new(stdout))
                .map_err(|e| format!("parse llvm-cov output (iter {idx}): {e}"))?;

        // Accumulate this snapshot's per-region hit count into a local map.
        let mut local: HashMap<RegionKey, u64> = HashMap::new();
        for data in export.data {
            for func in data.functions {
                if !func.name.contains(module) {
                    continue;
                }
                let fname = func.filenames.first().cloned().unwrap_or_default();
                for region in &func.regions {
                    if region.len() < 5 {
                        continue;
                    }
                    let key = RegionKey {
                        fname: fname.clone(),
                        func: func.name.clone(),
                        sl: region_u32(region, 0),
                        sc: region_u32(region, 1),
                        el: region_u32(region, 2),
                        ec: region_u32(region, 3),
                    };
                    *local.entry(key).or_insert(0) += region_u64(region, 4);
                }
            }
        }

        // Fold into global accumulator.
        let mut global = accum.lock().map_err(|_| "poisoned lock".to_string())?;
        for (key, count) in local {
            let entry = global.entry(key).or_default();
            if *is_neg {
                if count > 0 {
                    entry.ef += 1;
                }
                entry.neg_count += count;
            } else {
                if count > 0 {
                    entry.ep += 1;
                }
                entry.pos_count += count;
            }
        }
        Ok(())
    })?;

    let global = accum.into_inner().map_err(|_| "poisoned lock")?;

    // Build output regions in the same shape as crabcheck-profiling-analysis --print-json.
    let pos_len = (positive_samples as f64).max(1.0);
    let neg_len = (negative_samples as f64).max(1.0);
    let total_pos = positive_samples as u64;
    let total_neg = negative_samples as u64;

    let mut out_regions: Vec<Value> = global
        .into_iter()
        .map(|(key, c)| {
            let positive_avg = c.pos_count as f64 / pos_len;
            let negative_avg = c.neg_count as f64 / neg_len;
            let delta = negative_avg - positive_avg;
            let nf = total_neg.saturating_sub(c.ef);
            let np = total_pos.saturating_sub(c.ep);
            let susp = Suspiciousness::compute(c.ef, c.ep, nf, np);
            let demangled = format!("{:#}", demangle(&key.func));
            serde_json::json!({
                "file": key.fname,
                "function": demangled,
                "start_line": key.sl,
                "start_col": key.sc,
                "end_line": key.el,
                "end_col": key.ec,
                "positive_avg": positive_avg,
                "negative_avg": negative_avg,
                "delta": delta,
                "ef": c.ef,
                "ep": c.ep,
                "nf": nf,
                "np": np,
                "suspiciousness": {
                    "tarantula": susp.tarantula,
                    "ochiai": susp.ochiai,
                    "dstar": susp.dstar,
                    "jaccard": susp.jaccard,
                    "op2": susp.op2,
                }
            })
        })
        .collect();

    // Stable ordering for diff-ability: sort by (file, start_line, start_col, function).
    out_regions.sort_by(|a, b| {
        let ka = (
            a["file"].as_str().unwrap_or(""),
            a["start_line"].as_u64().unwrap_or(0),
            a["start_col"].as_u64().unwrap_or(0),
            a["end_line"].as_u64().unwrap_or(0),
            a["end_col"].as_u64().unwrap_or(0),
            a["function"].as_str().unwrap_or(""),
        );
        let kb = (
            b["file"].as_str().unwrap_or(""),
            b["start_line"].as_u64().unwrap_or(0),
            b["start_col"].as_u64().unwrap_or(0),
            b["end_line"].as_u64().unwrap_or(0),
            b["end_col"].as_u64().unwrap_or(0),
            b["function"].as_str().unwrap_or(""),
        );
        ka.cmp(&kb)
    });

    if print_json {
        let out = serde_json::json!({
            "positive_samples": positive_samples,
            "negative_samples": negative_samples,
            "regions": out_regions,
        });
        println!("{}", serde_json::to_string(&out)?);
    } else {
        // Human-readable — same header as analysis.rs, filter delta > 0.
        println!(
            "{:60} {:>8} {:>8} {:>8} {:>8}",
            "File:Line", "Pos", "Neg", "Δ", "Ochiai"
        );
        println!("{}", "-".repeat(93));
        for region in &out_regions {
            let delta = region["delta"].as_f64().unwrap_or(0.0);
            if delta <= 0.0 {
                continue;
            }
            let func = region["function"].as_str().unwrap_or("?");
            let file_full = region["file"].as_str().unwrap_or("?");
            let sl = region["start_line"].as_u64().unwrap_or(0);
            let sc = region["start_col"].as_u64().unwrap_or(0);
            let el = region["end_line"].as_u64().unwrap_or(0);
            let ec = region["end_col"].as_u64().unwrap_or(0);
            let pos_avg = region["positive_avg"].as_f64().unwrap_or(0.0);
            let neg_avg = region["negative_avg"].as_f64().unwrap_or(0.0);
            let ochiai = region
                .pointer("/suspiciousness/ochiai")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let fname = Path::new(file_full)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(file_full);
            let label = format!("({}){}:{}:{} -> {}:{}", func, fname, sl, sc, el, ec);
            println!(
                "{:60} {:>8.2} {:>8.2} {:+>8.2} {:>8.4}",
                label, pos_avg, neg_avg, delta, ochiai
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
