# locate-demo

A worked example showing `crabcheck_locate::quickcheck_with_locate!` end to end:
write a property, run it, get back the buggy file:line — no manual
post-processing, no separate analyzer binary.

## Setup

The example is a standalone crate in `crabcheck/examples/locate-demo/`. It
depends on `crabcheck` via path with the `profiling` feature enabled, and
`.cargo/config.toml` supplies the coverage instrumentation flags so you
don't have to remember `RUSTFLAGS=...` on every invocation.

You also need `llvm-cov` and `llvm-profdata`. Either:

```bash
rustup component add llvm-tools-preview      # ships them with the toolchain
# or
brew install llvm                             # macOS, system-wide
```

The example auto-detects them via `rustc --print sysroot` so neither
needs to be on `PATH`.

## Running it

From this directory:

```bash
cargo run
```

Expected output (input ranges intentionally trip the planted bug quickly):

```
Failed (passed=4, discarded=0)
Counterexample: Pair { x: 137, y: -42 }
Captured 800 panic events.

Top suspects:
  1. lib.rs:11-15 in `locate_demo::buggy_addition`  ochiai=1.00 Δ=18.4  HIGH panic-overlap (singleton_panic_overlap)
  2. lib.rs:32-36 in `<locate_demo::Pair as ...::Mutate<R>>::mutate`  ochiai=0.91 Δ= 4.1  MEDIUM
  ...
```

The `HIGH (singleton_panic_overlap)` band is the strongest confidence —
exactly one function in the workload contains all captured panic frames,
so it's the bug with very high probability.

## Running it as a `#[test]`

```bash
cargo test
```

`tests/locate.rs` calls `quickcheck_with_locate!` from a `#[test]` and
asserts the planted bug at `src/lib.rs:11–15` is the top suspect. The
report is printed to stderr (visible without `--nocapture`).

The orchestrator serializes concurrent calls inside the process via a
mutex, so parallel `cargo test` doesn't need `RUST_TEST_THREADS=1`.

**Important:** integration tests under `tests/` are compiled as their own
crate, named after the file (e.g. `tests/locate.rs` → crate `locate`).
`env!("CARGO_CRATE_NAME")` therefore resolves to `"locate"`, which would
filter the library's symbols out of the suspect ranking. Pass the
package's library name explicitly via the 2-arg macro form:

```rust
let report = crabcheck_locate::quickcheck_with_locate!(
    locate_demo::property_addition_total,
    "locate_demo"
);
```

In `main.rs` and unit tests inside `lib.rs`, the 1-arg form is fine —
`CARGO_CRATE_NAME` resolves to the package name there. See
`src/lib.rs::tests::unit_test_finds_planted_overflow_bug` for the
unit-test variant.

## When something's wrong

If you see `Configuration issue:` lines in the output, the orchestrator
detected a setup problem and is suggesting a fix. The diagnostics it
surfaces:

- **NoCoverageData** — your binary lacks the `__llvm_profile_*` runtime.
  Add the instrumentation `rustflags` to `.cargo/config.toml`.
- **LlvmToolMissing** — `llvm-cov` / `llvm-profdata` aren't on PATH and
  not in rustc's sysroot. Run `rustup component add llvm-tools-preview`.
- **ModuleFilterMatchedNothing** — the module name is wrong. Output
  includes a list of crate prefixes the binary contains; use one in the
  2-arg macro form.
- **NoCounterexample** — the property held for every iteration. Either
  the property is correct, or your input range is too narrow.

## What you write

```rust
fn main() {
    let report = crabcheck_locate::quickcheck_with_locate!(my_crate::property_foo);
    println!("{report}");
}
```

That's it. The macro auto-fills `module = env!("CARGO_CRATE_NAME")` so
crabcheck filters the suspect list to your crate's regions only.

## Pre-requisites in your own crate

1. Depend on `crabcheck` with the `profiling` feature.
2. Have `RUSTFLAGS=-C instrument-coverage` (and friends) at compile time —
   the easiest way is the `.cargo/config.toml` shown in this example.
3. Have `llvm-cov` and `llvm-profdata` on `PATH` *or* a working `rustup`.

That's the whole prerequisite list. No env-var dance, no separate post-
processing binary, no Python.
