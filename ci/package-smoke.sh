#!/usr/bin/env bash
# Build and test a consumer against the packaged .crate contents, the way a
# crates.io user sees them: no path dependencies, only what `include` ships.
# The macros crate is supplied through [patch.crates-io] because the packaged
# crabcheck depends on it by version.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/crates/crabcheck/Cargo.toml" | head -1)
macros_version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/crates/crabcheck-macros/Cargo.toml" | head -1)

(cd "$root" && cargo package -p crabcheck-macros -p crabcheck --allow-dirty --no-verify)

work="${SMOKE_DIR:-$(mktemp -d)}"
rm -rf "$work"
mkdir -p "$work/crates"
cp -R "$root/ci/package-smoke/." "$work/"
# Unpack the .crate files: that is exactly what a registry user gets.
tar -xzf "$root/target/package/crabcheck-$version.crate" -C "$work/crates"
tar -xzf "$root/target/package/crabcheck-macros-$macros_version.crate" -C "$work/crates"
pkg="$work/crates"
cat > "$work/Cargo.toml" <<TOML
[package]
name = "crabcheck-smoke"
version = "0.0.0"
edition = "2024"
publish = false

[dependencies]
crabcheck = { path = "$pkg/crabcheck-$version", features = ["coverage"] }

[patch.crates-io]
crabcheck-macros = { path = "$pkg/crabcheck-macros-$macros_version" }

[workspace]
TOML

cd "$work"
echo "== uninstrumented =="
cargo test
echo "== instrumented =="
RUSTFLAGS="-C instrument-coverage" RUST_TEST_THREADS=1 LLVM_PROFILE_FILE="$work/target/profraw/%m_%p.profraw" cargo test
echo "== docs of the packaged crate =="
RUSTDOCFLAGS="-D warnings" cargo doc -p crabcheck --no-deps
echo "package smoke test passed ($work)"
