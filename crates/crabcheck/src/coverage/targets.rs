//! Coverage targets: source ranges the search should reach.
//!
//! A target names a file and a line range. It is resolved through the
//! coverage mapping to the lines' innermost regions (llvm-cov's rule, so a
//! line inside an untaken branch does not count as reached because the
//! enclosing function ran), the functions that own those regions join the
//! tracked set, and an input that executes a target line counts as novel
//! for the guided search, so the pool climbs toward the target before
//! anyone rewrites a generator. The report says, per target, how many
//! inputs hit it and, when none did, whether its function was entered at
//! all.

use std::collections::{
    BTreeMap,
    BTreeSet,
};

use crate::config::CoverageTarget;

use super::{
    counters::{
        Delta,
        Functions,
    },
    locate::same_file,
    mapping::{
        Kind,
        Mapping,
    },
};

/// What happened to one coverage target over the run.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
pub struct TargetOutcome {
    /// The target as configured.
    pub target: CoverageTarget,
    /// Functions whose code lies in the target; empty when it matched no
    /// instrumented code.
    pub functions: Vec<String>,
    /// Inputs that executed a line inside the target.
    pub hits: u64,
    /// The observed-input index at which the target was first hit.
    pub first_hit_at: Option<u64>,
    /// Inputs that entered any of the target's functions.
    pub function_inputs: u64,
}

struct State {
    target: CoverageTarget,
    functions: Vec<usize>,
    /// (function index, file index, line) triples inside the target whose
    /// innermost region belongs to that function.
    lines: Vec<(usize, usize, u32)>,
    hits: u64,
    first_hit_at: Option<u64>,
    function_inputs: u64,
}

/// The configured targets, resolved against the mapping.
pub(crate) struct Targets {
    states: Vec<State>,
    /// Function index to the targets it belongs to.
    by_function: BTreeMap<usize, Vec<usize>>,
}

impl Targets {
    /// Resolve targets to lines and functions; `None` without a mapping.
    pub fn resolve(targets: Vec<CoverageTarget>) -> Option<Targets> {
        let mapping = Mapping::get()?;
        let all = Functions::get().all();
        let mut states = Vec::with_capacity(targets.len());
        let mut by_function: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for target in targets {
            let mut state = State {
                target,
                functions: Vec::new(),
                lines: Vec::new(),
                hits: 0,
                first_hit_at: None,
                function_inputs: 0,
            };
            let files: Vec<usize> = mapping
                .files
                .iter()
                .enumerate()
                .filter(|(_, f)| same_file(f, &state.target.file))
                .map(|(i, _)| i)
                .collect();
            if files.is_empty() {
                states.push(state);
                continue;
            }
            let range = state.target.begin_line..=state.target.end_line;
            for (fi, function) in all.iter().enumerate() {
                let Some(mapped) = mapping.function(function.name_ref, function.func_hash) else {
                    continue;
                };
                let overlaps = mapped.regions.iter().any(|region| {
                    matches!(region.kind, Kind::Code | Kind::Branch)
                        && files.contains(&region.file)
                        && region.start_line <= *range.end()
                        && *range.start() <= region.end_line
                });
                if !overlaps {
                    continue;
                }
                // The lines of the target this function owns; a nested closure
                // is a separate function with its own regions.
                let zero = vec![0u64; function.counters.len()];
                let mut any = false;
                for ((file, line), _) in mapped.line_counts(&zero) {
                    if files.contains(&file) && range.contains(&line) {
                        state.lines.push((fi, file, line));
                        any = true;
                    }
                }
                if any {
                    state.functions.push(fi);
                    by_function.entry(fi).or_default().push(states.len());
                }
            }
            states.push(state);
        }
        Some(Targets { states, by_function })
    }

    /// The functions that own target lines, for the tracked set.
    pub fn functions(&self) -> Vec<usize> {
        self.by_function.keys().copied().collect()
    }

    /// The targets that matched no instrumented code.
    pub fn unresolved(&self) -> impl Iterator<Item = &CoverageTarget> + '_ {
        self.states.iter().filter(|s| s.functions.is_empty()).map(|s| &s.target)
    }

    /// Note an input; returns whether it hit a target line.
    pub fn observe(&mut self, delta: &Delta, input: u64) -> bool {
        let Some(mapping) = Mapping::get() else {
            return false;
        };
        let all = Functions::get().all();
        let hits = delta.hits();
        let mut entered: BTreeSet<usize> = BTreeSet::new();
        let mut hit: BTreeSet<usize> = BTreeSet::new();
        let mut values = Vec::new();
        for (&fi, owners) in &self.by_function {
            let range = &all[fi].counters;
            let first = hits.partition_point(|(i, _)| *i < range.start);
            if first >= hits.len() || hits[first].0 >= range.end {
                continue;
            }
            let Some(mapped) = mapping.function(all[fi].name_ref, all[fi].func_hash) else {
                continue;
            };
            values.clear();
            values.resize(range.len(), 0);
            for &(i, count) in &hits[first..] {
                if i >= range.end {
                    break;
                }
                values[i - range.start] = count;
            }
            let counts = mapped.line_counts(&values);
            for &si in owners {
                entered.insert(si);
                let reached = self.states[si].lines.iter().any(|&(f, file, line)| {
                    f == fi && counts.get(&(file, line)).is_some_and(|c| *c > 0)
                });
                if reached {
                    hit.insert(si);
                }
            }
        }
        for si in entered {
            self.states[si].function_inputs += 1;
        }
        for &si in &hit {
            let state = &mut self.states[si];
            state.hits += 1;
            state.first_hit_at.get_or_insert(input);
        }
        !hit.is_empty()
    }

    /// The outcome of every target.
    pub fn outcomes(&self) -> Vec<TargetOutcome> {
        let all = Functions::get().all();
        self.states
            .iter()
            .map(|s| {
                TargetOutcome {
                    target: s.target.clone(),
                    functions: s.functions.iter().map(|&fi| all[fi].name.clone()).collect(),
                    hits: s.hits,
                    first_hit_at: s.first_hit_at,
                    function_inputs: s.function_inputs,
                }
            })
            .collect()
    }
}
