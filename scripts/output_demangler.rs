//! Post-process `crabcheck-profiling-analysis --print-json` (or
//! `crabcheck-profiling-fast-analyze --print-json`) output: walk
//! `.regions[].function` and replace each mangled Rust symbol with its
//! demangled form via `rustc_demangle::demangle`.
//!
//! Reads stdin (or a file argument), writes to stdout. Intended as a tail
//! pipe stage after the analysis binary, so we can skip per-snapshot
//! demangling entirely and demangle only the ~500 KB final output.

#[cfg(feature = "profiling")]
use {
    rustc_demangle::demangle,
    serde_json::{Map, Value},
    std::{
        env,
        io::{self, Read, Write},
    },
};

#[cfg(feature = "profiling")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();

    let mut buf = String::new();
    if args.len() >= 2 {
        buf = std::fs::read_to_string(&args[1])?;
    } else {
        io::stdin().read_to_string(&mut buf)?;
    }

    let mut root: Value = serde_json::from_str(&buf)?;
    demangle_regions(&mut root);

    let out = serde_json::to_string(&root)?;
    io::stdout().write_all(out.as_bytes())?;
    io::stdout().write_all(b"\n")?;
    Ok(())
}

#[cfg(feature = "profiling")]
fn demangle_regions(root: &mut Value) {
    let Some(obj) = root.as_object_mut() else { return };
    let Some(regions) = obj.get_mut("regions") else { return };
    let Some(arr) = regions.as_array_mut() else { return };
    for region in arr {
        if let Some(region_obj) = region.as_object_mut() {
            demangle_function_field(region_obj);
        }
    }
}

#[cfg(feature = "profiling")]
fn demangle_function_field(region: &mut Map<String, Value>) {
    let Some(f) = region.get("function") else { return };
    let Some(name) = f.as_str() else { return };
    let demangled = format!("{:#}", demangle(name));
    region.insert("function".to_string(), Value::String(demangled));
}

#[cfg(not(feature = "profiling"))]
fn main() {
    eprintln!("This binary requires the 'profiling' feature to be enabled.");
    std::process::exit(1);
}
