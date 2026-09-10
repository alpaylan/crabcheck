# crabcheck

Property-based testing for Rust, built for tests that are **written and
verified by coding agents**: you author the generator, the support
predicate, the shrinker and the mutator; the library verifies all of them
and makes every failure reproducible from its seed.

```rust
use crabcheck::prelude::*;

#[crabcheck::test]
fn sorting_keeps_every_element(mut v: Vec<i32>) -> bool {
    let mut expected = v.clone();
    expected.sort();
    v.sort();
    v == expected
}
```

A failure prints everything needed to act on it:

```text
crabcheck: FAILED, property returned false
  counterexample: [0, -1]
  original:       [1000, -268, -1000, 341, 0] (size 5, iteration 13)
  shrinking: 6 of 41 candidates accepted in 0.4 ms
  after 13 passing tests (0 discarded) in 1.4 ms
  reproduce: CRABCHECK_REPLAY=13538456636133602880:13
  or rerun the whole search: Config::default().seed(13538456636133602880)
  remembered in .crabcheck/regressions/insertion_sort.jsonl
```

## The idea

Most property-testing libraries hide shrinking inside the generator so that
nobody has to write a shrinker. crabcheck takes the opposite position:
shrinkers and mutators are ordinary code that an agent can write in a
minute, and their correctness is a property the library checks for you.

- **Generators are explicit and uniform.** `Source` is a seeded random
  source with uniform primitives. Edge-case bias is composed from visible
  generators (`gens::edges`, `gens::f64_bits`, ...), and `any::<T>()` is a
  documented `frequency` mix you can rebuild or replace.
- **Every generator owns its shrinker and mutator.** Built-ins carry hooks
  that stay in their range. For your own generators, attach candidates
  with `with_shrink` / `with_mutate`, use a bijection with `iso`, or derive
  structural hooks with `#[derive(Arbitrary)]`.
- **Correctness is verifiable.** `check_shrinker(gen, support, size)`
  checks that every candidate stays in the generator's support and is
  strictly smaller; `check_mutator` checks mutants; `check_generator`
  checks the support itself. They run through `check`, so they shrink and
  replay like any other property.
- **Everything is reproducible.** A failure is `seed:iteration`;
  `CRABCHECK_REPLAY` regenerates it without searching. Failures are
  remembered in a regressions file that is replayed first next time and
  flags entries whose generator has changed.
- **Everything is configurable and machine-readable.** Every setting is a
  `Config` field, a `CRABCHECK_*` variable, or a JSON file, and `Report`
  is JSON as well as text.
- **Targeted search.** Call `target(score)` inside a property and the
  runner mutates the best-scoring inputs with the generator's mutator to
  push the score up.

## Writing generators

```rust
use crabcheck::{gens, Gen, GenExt, Source, check_shrinker, Config};

#[derive(Debug, Clone, PartialEq)]
enum Tree { Leaf, Node(Box<Tree>, i32, Box<Tree>) }

// Valid by construction: each key is drawn from the range its position allows.
fn bst(g: &mut Source, lo: i32, hi: i32, depth: usize) -> Tree {
    if depth == 0 || lo > hi || !g.bool_with(0.8) {
        return Tree::Leaf;
    }
    let key = g.int(lo..=hi);
    Tree::Node(Box::new(bst(g, lo, key - 1, depth - 1)), key, Box::new(bst(g, key + 1, hi, depth - 1)))
}

// A shrinker that keeps the invariant: either child, a leaf, or a node with one child shrunk.
fn shrink_bst(t: &Tree) -> Vec<Tree> {
    match t {
        Tree::Leaf => Vec::new(),
        Tree::Node(l, k, r) => {
            let mut out = vec![(**l).clone(), (**r).clone(), Tree::Leaf];
            out.extend(shrink_bst(l).into_iter().map(|l| Tree::Node(Box::new(l), *k, r.clone())));
            out.extend(shrink_bst(r).into_iter().map(|r| Tree::Node(l.clone(), *k, Box::new(r))));
            out
        }
    }
}

fn is_bst(t: &Tree) -> bool { /* in-order keys strictly increasing */ true }
fn size(t: &Tree) -> u64 { match t { Tree::Leaf => 0, Tree::Node(l, _, r) => 1 + size(l) + size(r) } }

let trees = (|g: &mut Source| bst(g, -100, 100, 6)).with_shrink(shrink_bst).boxed();
check_shrinker(&Config::default(), trees.clone(), is_bst, size).assert_passed();
```

`#[derive(Arbitrary)]` covers plain structs and enums, with
`#[crabcheck(gen = ...)]` per field and `#[crabcheck(support = ...)]` per
type; `any::<T>()` gives the default generator of any std type.

## Checking properties

```rust
use crabcheck::{check, gens, Config, TestResult, classify, target};

let report = check(&Config::default().tests(500), gens::vec(gens::int(-50i32..=50)), |v| {
    classify(v.is_empty(), "empty");
    target(v.len() as f64);           // optional: steer toward long inputs
    let sum: i32 = v.iter().sum();
    TestResult::check(sum.abs() <= 50 * v.len() as i32, "sum out of bounds")
});
report.assert_passed();
```

Properties may return `bool`, `()`, `Result<_, E: Debug>` or `TestResult`;
a panic is a failure with its message and location captured. `assume!`
discards inputs that do not apply.

## Reproducing and steering runs

| Variable | Effect |
|---|---|
| `CRABCHECK_REPLAY=seed:iteration` | regenerate and test one input first |
| `CRABCHECK_SEED=N` | rerun the same search |
| `CRABCHECK_TESTS=N`, `CRABCHECK_BUDGET_MS=N`, `CRABCHECK_MAX_SIZE=N` | change the budget |
| `CRABCHECK_SHRINK=0` | report the original failing input |
| `CRABCHECK_TARGET_TESTS=N` | budget of the targeted phase |
| `CRABCHECK_REPORT_JSON=path` | append every report as a JSON line |
| `CRABCHECK_CONFIG=path.json` | any subset of `Config` fields |

The full list is in the `config` module documentation.

## Features

- `json` (default): JSON reports and config, the regressions file.
- `derive` (default): `#[derive(Arbitrary)]` and `#[crabcheck::test]`.
- `coverage`: in-process coverage for binaries built with
  `RUSTFLAGS="-C instrument-coverage"`: per-property coverage in the
  report, coverage-guided search, `cover()` shares, fault localization
  (suspect functions ranked from the coverage of passing and failing runs)
  and coverage targets: name a file and line range in
  `.crabcheck/targets.jsonl` and the search steers toward it and the
  report says whether it got there. `CRABCHECK_COVERAGE_HTML=dir` writes a
  coverage grid page per property: a zoomable map of modules, functions
  and lines sized by instrumented lines, a source view with per-line
  counts, the targets and sampled inputs, where a line range becomes a
  new target and a brief for the agent.
- `serve`: the `crabcheck serve` command, a local page over `.crabcheck`
  (grid pages, latest reports, targets) with live reload and an
  "Add to targets.jsonl" button on served grid pages.
  Uninstrumented builds compile the same API to no-ops. Run coverage-using
  suites with `RUST_TEST_THREADS=1` for exact attribution, and point
  `LLVM_PROFILE_FILE` into the target directory (see the `coverage` module
  docs) so instrumented processes do not leave `default_*.profraw` files
  behind. On nightly,
  `-Z coverage-options=branch` adds per-outcome branch counters that the
  same report and localization use.

Set `CRABCHECK_OBSERVATIONS=.crabcheck/observed` to write one observation
file per property in the OpenPBTStats format, and open it in
[Tyche](https://github.com/tyche-pbt/tyche-extension) to see the sample
breakdown, feature distributions, coverage and timing charts.

Minimum supported Rust version: 1.88.

## Status

0.2 is a rewrite of the 0.1 research prototype and its API is not yet
frozen. Coverage-guided search and fault localization now live in this
crate behind the `coverage` feature; the 0.1 file-based pipeline in
`crates/crabcheck-locate` is superseded and kept only for reference.

Planned: PerfFuzz-style search, where inputs that set a new maximum
execution count on any counter are kept and the total path length is the
score when the property reports none, with the hottest input and functions
in the report. Today coverage-guided search rewards new code only (AFL
hit-count classes saturate at 128), and performance search needs a
hand-written `target` score.

## License

MIT.
