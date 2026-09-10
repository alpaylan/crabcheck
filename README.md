# crabcheck

Property-based testing for Rust, designed for tests written by coding
agents: one generator gives generation, shrinking, targeted search and exact
replay; every report is readable by people and by tools; every setting can
be changed from the environment.

See [`crates/crabcheck/README.md`](crates/crabcheck/README.md) for the
library's own README and the [changelog](crates/crabcheck/CHANGELOG.md).

## Workspace

| Crate | Purpose | Status |
|---|---|---|
| [`crates/crabcheck`](crates/crabcheck) | The library: generators with verified shrinkers and mutators, targeted and coverage-guided search, seed replay, regressions, JSON reports, parallel runs, in-process coverage and fault localization | 0.2 (API not yet frozen) |
| [`crates/crabcheck-macros`](crates/crabcheck-macros) | `#[derive(Arbitrary)]` and `#[crabcheck::test]`, re-exported by `crabcheck` | 0.2 |
| [`crates/crabcheck-locate`](crates/crabcheck-locate) | The 0.1 file-based fault-localization pipeline | Superseded by `crabcheck`'s `coverage` feature; kept for reference, unpublished |
| [`examples/locate-demo`](examples/locate-demo) | Standalone example of fault localization on the 0.2 core | Builds against `crabcheck` with `-C instrument-coverage` |

## Developing

```bash
cargo test --workspace                                   # stable toolchain, MSRV 1.88
cargo clippy --workspace --all-targets -- -D warnings
cargo +nightly fmt --all                                 # rustfmt config uses nightly-only options
cargo run -p crabcheck --example bst -- --bug            # see a failure report
cargo run -p crabcheck --example sort_performance        # see the targeted phase
```

## License

MIT. See [LICENSE](LICENSE).
