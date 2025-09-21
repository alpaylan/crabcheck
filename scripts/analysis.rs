use {
    rayon::prelude::*,
    serde::Deserialize,
    serde_json::Value,
    std::{
        collections::{
            BTreeSet,
            HashMap,
        },
        fs::File,
        io::BufReader,
        path::{
            Path,
            PathBuf,
        },
    },
};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct RegionKey {
    fname: String,
    func: String,
    sl: u32,
    sc: u32,
    el: u32,
    ec: u32,
}

#[derive(Debug, Deserialize)]
struct Indices {
    positives: Vec<usize>,
    negatives: Vec<usize>,
}

fn snapshot_path(i: usize) -> PathBuf {
    PathBuf::from(format!("jsondata/demangled/snapshot_iteration_{i}.json"))
}

fn number_to_u64(v: &Value) -> u64 {
    match v {
        Value::Number(n) => {
            if let Some(u) = n.as_u64() {
                u
            } else if let Some(i) = n.as_i64() {
                i.max(0) as u64
            } else if let Some(f) = n.as_f64() {
                if f.is_sign_negative() { 0 } else { f as u64 }
            } else {
                0
            }
        },
        _ => 0,
    }
}

fn number_to_u32(v: &Value) -> u32 {
    number_to_u64(v) as u32
}

fn extract_region_counts(json_path: &Path, module: &str) -> HashMap<RegionKey, u64> {
    let file = match File::open(json_path) {
        Ok(f) => f,
        Err(_) => return HashMap::new(),
    };
    let reader = BufReader::new(file);
    let root: Value = match serde_json::from_reader(reader) {
        Ok(v) => v,
        Err(_) => return HashMap::new(),
    };

    // Navigate: data[0].functions[*]
    let mut regions: HashMap<RegionKey, u64> = HashMap::new();
    let Some(data_arr) = root.get("data").and_then(|v| v.as_array()) else {
        return regions;
    };
    let Some(entry) = data_arr.get(0) else {
        return regions;
    };
    let Some(functions) = entry.get("functions").and_then(|v| v.as_array()) else {
        return regions;
    };

    for func in functions {
        let Some(name) = func.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        if !name.contains(module) {
            continue;
        }
        let Some(filenames) = func.get("filenames").and_then(|v| v.as_array()) else {
            continue;
        };
        let Some(fname0) = filenames.get(0).and_then(|v| v.as_str()) else {
            continue;
        };
        let fname = fname0.to_string();

        // regions is an array of arrays; we only need first 5 fields:
        // [start_line, start_col, end_line, end_col, count, ...]
        let Some(regs) = func.get("regions").and_then(|v| v.as_array()) else {
            continue;
        };

        for reg in regs {
            let Some(items) = reg.as_array() else {
                continue;
            };
            if items.len() < 5 {
                continue;
            }

            let sl = number_to_u32(&items[0]);
            let sc = number_to_u32(&items[1]);
            let el = number_to_u32(&items[2]);
            let ec = number_to_u32(&items[3]);
            let count = number_to_u64(&items[4]);

            let key = RegionKey { fname: fname.clone(), func: name.to_string(), sl, sc, el, ec };
            *regions.entry(key).or_insert(0) += count;
        }
    }

    regions
}

fn aggregate(paths: &[PathBuf], module: &str) -> HashMap<RegionKey, u64> {
    // Parallel parse, then reduce into a single map
    paths.par_iter().map(|p| extract_region_counts(p, module)).reduce(HashMap::new, |mut acc, m| {
        for (k, v) in m {
            *acc.entry(k).or_insert(0) += v;
        }
        acc
    })
}

fn main() {
    // get `--json-path` argument
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: crabcheck-profiling-analysis <coverage_data_path> <module>");
        std::process::exit(1);
    }
    let coverage_data_path = &args[1];
    let module = &args[2];

    let indices_path = PathBuf::from(format!("{}/indices.json", coverage_data_path));
    let file = File::open(&indices_path).expect("Failed to open indices.json");
    let indices: Indices =
        serde_json::from_reader(BufReader::new(file)).expect("Failed to parse indices.json");
    let positive_paths: Vec<PathBuf> = indices.positives.into_iter().map(snapshot_path).collect();
    let negative_paths: Vec<PathBuf> = indices.negatives.into_iter().map(snapshot_path).collect();

    let pos_cov = aggregate(&positive_paths, module);
    let neg_cov = aggregate(&negative_paths, module);

    let pos_len = positive_paths.len().max(1) as f64; // avoid div-by-zero
    let neg_len = negative_paths.len().max(1) as f64;

    // Union of all regions, sorted for stable output
    let mut all_regions: BTreeSet<RegionKey> = BTreeSet::new();
    all_regions.extend(pos_cov.keys().cloned());
    all_regions.extend(neg_cov.keys().cloned());

    println!("{:60} {:>8} {:>8} {:>8}", "File:Line", "Pos", "Neg", "Δ");
    println!("{}", "-".repeat(84));

    for region in all_regions {
        let pos_avg = pos_cov.get(&region).map(|&v| v as f64 / pos_len).unwrap_or(0.0);
        let neg_avg = neg_cov.get(&region).map(|&v| v as f64 / neg_len).unwrap_or(0.0);
        let delta = neg_avg - pos_avg;

        if delta > 0.0 {
            let label = format!(
                "({}){}:{}:{} -> {}:{}",
                region.func,
                Path::new(&region.fname)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(&region.fname),
                region.sl,
                region.sc,
                region.el,
                region.ec
            );
            println!("{:60} {:>8.2} {:>8.2} {:+>8.2}", label, pos_avg, neg_avg, delta);
        }
    }
}
