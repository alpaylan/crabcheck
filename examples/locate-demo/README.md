# locate-demo

Fault localization end to end: a property fails, crabcheck shrinks the
input and reports which functions most likely contain the bug.

```bash
cd examples/locate-demo
cargo run            # prints the report with the suspect list
cargo test           # asserts that `buggy_addition` is the top suspect
```

The crate is standalone. Its `.cargo/config.toml` sets
`-C instrument-coverage`, crabcheck's build script sees the flag, and the
`coverage` feature reads the profile counters and the coverage mapping in
process. No LLVM tools are needed. The same file points `LLVM_PROFILE_FILE`
into `target/profraw/` so the profile files every instrumented process
writes at exit stay out of the source tree.

```
crabcheck: FAILED, property panicked: planted overflow at x > 100
  at src/lib.rs:20:9
  counterexample: Pair { x: 101, y: 0 }
  original:       Pair { x: 180, y: -96 } (size 31, iteration 2)
  shrinking: 4 of 23 candidates accepted in 0.4 ms
  ...
  coverage: 3 of 8 functions in `locate_demo` reached, 6 of 10 counters
  suspects (37 passing and 50 failing runs over 20 regions):
    1. src/lib.rs:18-23 in `locate_demo::buggy_addition`  ochiai=1.00 delta=1.00  HIGH panic-site (singleton_panic_overlap)
    2. src/lib.rs:41-43 in `locate_demo::addition_is_total`  ochiai=0.76 delta=0.00  LOW_MEDIUM (default)
    3. src/lib.rs:27-29 in `locate_demo::honest_addition`  ochiai=0.00 delta=-1.00  LOW_MEDIUM (default)
```

The spectrum holds the passing inputs of the random phase, every input the
shrinker tried, and mutants of the shrunk input; `HIGH (singleton_panic_overlap)`
means exactly one ranked function contains the panic site.

`check_addition` in `src/lib.rs` sets `config.coverage.filter` to the
library's crate name because the property may run from an integration test,
whose own crate would otherwise be the default filter.
