# Changelog

All notable changes to `crabcheck` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased] — 0.2.0

A rewrite of the 0.1 research prototype around one idea: the author writes
the generator, the support predicate, the shrinker and the mutator, and the
library verifies them and makes every failure reproducible from its seed.

### Added
- `Source`, a seeded random source (`RngCore`) with uniform primitives
  (`int`, `f64`, `f64_bits`, `bool`, `pick`, `weighted`) and a size
  parameter; generators are deterministic in the seed.
- `Gen` with three methods: `generate`, `shrink` (simpler candidates) and
  `mutate` (a nearby variant). Closures are generators; combinators `map`,
  `iso` (a bijection that carries hooks), `and_then`, `filter`,
  `with_shrink`, `with_mutate`, `typed` (use the value type's hooks),
  `collect`, `boxed`. Tuples and arrays of generators are generators.
- Built-in generators in `crabcheck::gens`, each with hooks that stay in
  its range: integers, `edges`, `pick_from`, floats (unit interval, raw
  bit patterns, ranges, finite), chars and strings, `vec` with length
  bounds, `option`, `result`, maps and sets, `one_of`, `frequency`,
  `recursive`, `sized`, `constant`.
- Type-level `Shrink` and `Mutate` traits implemented for the std types;
  `Arbitrary` defaults documented as visible mixes
  (`frequency([(1, edges), (7, uniform)])` for integers; specials, small
  round numbers, the unit interval and raw bits for floats).
- Checkers: `check_generator`, `check_shrinker` (candidates inside the
  support and strictly smaller; the greedy chain reaches a fixpoint) and
  `check_mutator` (mutants inside the support; a warning for inert
  mutators).
- Runner: `check(config, generator, property)`, `quickcheck(property)`,
  `check_par` over `Config::threads`. Properties return `bool`, `()`,
  `Result<_, E: Debug>` or `TestResult`; panics are captured with message
  and location and never printed while shrinking. `label`/`classify`
  record input distribution; `assume!` discards. A failure is re-run once
  (flakiness), shrunk with the generator's shrinker, and reported with the
  original and shrunk inputs, `ShrinkStats`, and the seed and iteration
  that regenerate it (`CRABCHECK_REPLAY=seed:iteration`).
- Regressions: failing inputs are remembered in
  `.crabcheck/regressions/<name>.jsonl` by seed and iteration and
  regenerated first on the next run; an entry whose regenerated input no
  longer renders the same is reported as stale.
- Targeted testing: a property calls `target(score)`; the random phase is
  followed by a phase that mutates the best-scoring inputs with the
  generator's mutator (`Config::target`), reported in `Report::target`.
- `Config` with public fields, chainable setters, JSON, and `CRABCHECK_*`
  environment overrides applied by `check` (bad values become
  `config_env` warnings). Defaults: 10 000 tests, `max_size` 100, sizes
  drawn per input (`max_size` one time in sixteen, else
  `floor(max_size * u²)`), optional `budget_ms`.
- `Report`: `Display` and JSON, with timing, distinct-input ratio,
  label/size/discard histograms and health warnings (discard ratio,
  degenerate generators, slow generation, sizes never reached, shrink
  budget, flakiness, configuration problems). `CRABCHECK_REPORT_JSON`
  appends reports as JSON lines.
- `coverage` feature: in-process LLVM coverage counters for binaries built
  with `-C instrument-coverage`. A build script detects the flag and sets
  `cfg(crabcheck_instrumented)` (the `coverage-force` feature overrides
  it); uninstrumented builds compile the same API to no-ops.
  `coverage::snapshot` copies the counter section with atomic loads,
  `Snapshot::delta` gives the counters an input touched, and
  `coverage::Functions` maps counters to demangled function names from
  the profile data and names sections. `check` collects coverage per
  input (snapshot, run, diff; never a counter reset) under a process-wide
  lock and reports functions reached, partly covered and never reached,
  counters hit and when coverage last grew (`Report::coverage`;
  `Config::coverage.filter` selects the crate under test by name prefix
  and defaults to the crate that authored the property). Health warnings:
  `coverage_threads` when `RUST_TEST_THREADS` is not 1,
  `coverage_plateau` when the generator stopped reaching new code,
  `coverage_unavailable` when a coverage option is requested in an
  uninstrumented build. `Config::coverage.profraw_dir` writes one
  `.profraw` per property for `llvm-cov`/`cargo llvm-cov`.
  `coverage::suppress_default_dump` (opt-in) stops the process's own
  `default_*.profraw` dump at exit; the module docs show the
  `[env] LLVM_PROFILE_FILE` setting that keeps every profile file, from
  test binaries and from `rustc` running instrumented proc macros, in the
  target directory.
- Coverage-guided search (`Config::coverage.guided`,
  `CRABCHECK_COVERAGE_GUIDED=1`): inputs that reach a counter or a
  hit-count class (AFL buckets) never seen before seed the targeted
  phase's pool, so the value mutators explore from them even when the
  property reports no `target` score.
- Fault localization in core (`Config::locate`, on by default when
  coverage is collected): the coverage mapping (`__llvm_covmap` and
  `__llvm_covfun`, versions 4 to 7) is read from the running executable,
  every run's counters are turned into region hits, and on failure the
  passing runs, the shrink trajectory and a mutant neighbourhood of the
  shrunk input form a pass/fail spectrum. Functions are ranked by Ochiai
  and count-mean difference with a prior for the function containing the
  panic site; `Report::locate` carries the suspects with all scores
  (Tarantula, Ochiai, D*, Jaccard, Op2, delta) and a confidence band, and
  the report prints the top ones. This replaces the profraw-per-input
  pipeline of `crabcheck-locate` (seconds per run instead of minutes).
  Nightly's `-Z coverage-options=branch` adds true/false branch regions
  that the spectrum scores as separate arms.
- `cover(condition, label)`: records how often a condition held across
  passing tests; `Report::covers` carries the shares and a `cover`
  warning fires when a label is met by fewer than
  `Config::coverage.min_cover` of the tests.
- `gens::dependent(outer, |a| inner)` (and `GenExt::dependent`): a pair
  whose second value is drawn from a generator chosen by the first. Both
  values are kept, so the pair shrinks and mutates within the joint
  support (`b` under `f(&a)`; `a` under `outer` with `b` regenerated
  from `f(&a')`). This is how generation depends on computation without
  losing hooks; `and_then` remains the generate-only bind.
- `crabcheck-macros` (default `derive` feature): `#[derive(Arbitrary)]`
  with `#[crabcheck(gen = ...)]` field overrides, and `#[crabcheck::test]`.

### Changed
- Repository reorganised into a Cargo workspace. The 0.1 code (random
  testing plus coverage-based fault localization) lives in
  `crates/crabcheck-locate` until it is rebuilt on the 0.2 core.

### Removed
- The 0.1 API: `Arbitrary<R: Rng>`, `Mutate<R>`, `quickcheck(fn(T) ->
  Option<bool>)`, `RunResult`, `Implies`, the four fuzz/targeting loops,
  `par_quickcheck`, `with_time`.
