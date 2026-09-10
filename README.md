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
| [`crates/crabcheck`](crates/crabcheck) | The library: generators, choice-tree shrinking, targeted mutation, replay tokens, regressions, JSON reports, parallel runs | 0.2 (API not yet frozen) |
| [`crates/crabcheck-macros`](crates/crabcheck-macros) | `#[derive(Arbitrary)]` and `#[crabcheck::test]`, re-exported by `crabcheck` | 0.2 |
| [`crates/crabcheck-locate`](crates/crabcheck-locate) | Coverage-guided fault localization (the 0.1 research code) | Unpublished; to be ported onto the 0.2 core |
| [`examples/locate-demo`](examples/locate-demo) | Standalone example of the fault-localization flow | Builds against `crabcheck-locate` |

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
