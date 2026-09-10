//! Detects whether this build is compiled with `-C instrument-coverage`
//! and, if so, sets `cfg(crabcheck_instrumented)` so the coverage module
//! can link against the LLVM profile runtime. Without the flag those
//! symbols do not exist and an `extern` block would fail to link, which is
//! why the detection happens here rather than at runtime.
//!
//! Cargo exposes the rustflags of the final build to build scripts as
//! `CARGO_ENCODED_RUSTFLAGS` (from `RUSTFLAGS` and `.cargo/config.toml`
//! `[build] rustflags`, also under `--target`). It does not include the
//! unstable `[profile.*.rustflags]` or flags passed to `cargo rustc --`;
//! the `coverage-force` feature covers those cases.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(crabcheck_instrumented)");
    println!("cargo::rerun-if-env-changed=CARGO_ENCODED_RUSTFLAGS");
    let forced = std::env::var_os("CARGO_FEATURE_COVERAGE_FORCE").is_some();
    if forced || instrumented(std::env::var("CARGO_ENCODED_RUSTFLAGS").unwrap_or_default()) {
        println!("cargo::rustc-cfg=crabcheck_instrumented");
    }
}

/// Whether the encoded rustflags enable coverage instrumentation.
fn instrumented(encoded: String) -> bool {
    let flags: Vec<&str> = encoded.split('\x1f').collect();
    let mut enabled = false;
    let mut i = 0;
    while i < flags.len() {
        let flag = flags[i];
        let value = if let Some(rest) = flag.strip_prefix("-Cinstrument-coverage") {
            Some(rest)
        } else if flag == "-C"
            && flags.get(i + 1).is_some_and(|next| next.starts_with("instrument-coverage"))
        {
            i += 1;
            Some(&flags[i]["instrument-coverage".len()..])
        } else {
            None
        };
        if let Some(value) = value {
            let value = value.trim_start_matches('=').to_ascii_lowercase();
            enabled = !matches!(value.as_str(), "no" | "off" | "false" | "n" | "0");
        }
        i += 1;
    }
    enabled
}
