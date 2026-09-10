# crabcheck

Property-based testing for Rust, designed for tests written by coding agents.

This repository is a Cargo workspace:

| Crate | Purpose | Status |
|---|---|---|
| [`crates/crabcheck`](crates/crabcheck) | The library: generators, choice-tree shrinking, targeted mutation, seed replay, JSON reports | 0.2 rewrite in progress |
| [`crates/crabcheck-locate`](crates/crabcheck-locate) | Coverage-guided fault localization (the 0.1.0 research code) | Unpublished; to be ported onto the 0.2 core |
| [`examples/locate-demo`](examples/locate-demo) | Standalone example of the fault-localization flow | Builds against `crabcheck-locate` |

## Developing

```bash
cargo test --workspace                       # stable toolchain
cargo clippy --workspace --all-targets -- -D warnings
cargo +nightly fmt --all                     # rustfmt config uses nightly-only options
```

## License

MIT. See [LICENSE](LICENSE).
