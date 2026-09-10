//! Spectrum-based fault localization over per-input region coverage.
//!
//! Every evaluated input adds one row to a pass/fail spectrum whose columns
//! are the source regions of the tracked functions. When a property fails,
//! the regions are scored with the usual formulas (Ochiai, Tarantula, D*,
//! Jaccard, Op2, count-mean difference) and ranked function by function,
//! with a prior for the function containing the panic site.

use std::{
    collections::{
        BTreeMap,
        BTreeSet,
        HashMap,
    },
    ops::Range,
};

use super::{
    counters::{
        Delta,
        Function,
        Functions,
    },
    mapping::{
        Kind,
        Mapping,
    },
};

/// SBFL scores of one region or function.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "json", derive(serde::Serialize))]
pub struct Scores {
    /// Tarantula: share of failing runs among all runs that hit the region.
    pub tarantula: f64,
    /// Ochiai: `ef / sqrt(f * (ef + ep))`, the primary ranking metric.
    pub ochiai: f64,
    /// D* (star 2): `ef² / (ep + nf)`.
    pub dstar: f64,
    /// Jaccard: `ef / (ef + nf + ep)`.
    pub jaccard: f64,
    /// Op2: `ef - ep / (p + 1)`.
    pub op2: f64,
    /// Mean count in failing runs minus mean count in passing runs.
    pub delta: f64,
}

impl Scores {
    /// From the spectrum counts: `ef`/`ep` failing/passing runs that hit
    /// the region, `nf`/`np` those that did not.
    pub fn compute(ef: u64, ep: u64, nf: u64, np: u64, delta: f64) -> Scores {
        let (ef, ep, nf, np) = (ef as f64, ep as f64, nf as f64, np as f64);
        let f = ef + nf;
        let p = ep + np;
        let tarantula = {
            let ef_over_f = if f > 0.0 { ef / f } else { 0.0 };
            let ep_over_p = if p > 0.0 { ep / p } else { 0.0 };
            let denom = ef_over_f + ep_over_p;
            if denom > 0.0 { ef_over_f / denom } else { 0.0 }
        };
        let ochiai = {
            let denom = (f * (ef + ep)).sqrt();
            if denom > 0.0 { ef / denom } else { 0.0 }
        };
        let dstar = {
            let denom = ep + nf;
            if denom > 0.0 {
                (ef * ef) / denom
            } else if ef > 0.0 {
                f64::MAX
            } else {
                0.0
            }
        };
        let jaccard = {
            let denom = ef + nf + ep;
            if denom > 0.0 { ef / denom } else { 0.0 }
        };
        let op2 = ef - ep / (p + 1.0);
        Scores { tarantula, ochiai, dstar, jaccard, op2, delta }
    }
}

/// How much to trust a suspect's rank.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize))]
pub enum Confidence {
    /// The only function containing the panic site, or both Ochiai and the
    /// count difference rank it first.
    High,
    /// One of Ochiai and the count difference ranks it first.
    Medium,
    /// Several functions contain panic sites.
    MediumLow,
    /// No strong signal either way.
    LowMedium,
    /// Ochiai and the count difference disagree by more than five ranks.
    Low,
}

impl std::fmt::Display for Confidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Confidence::High => "HIGH",
            Confidence::Medium => "MEDIUM",
            Confidence::MediumLow => "MEDIUM_LOW",
            Confidence::LowMedium => "LOW_MEDIUM",
            Confidence::Low => "LOW",
        })
    }
}

/// One ranked function: the regions of a function merged, with the widest
/// line range and the strongest scores among them.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "json", derive(serde::Serialize))]
pub struct Suspect {
    /// 1-based rank.
    pub rank: usize,
    /// Source file as recorded by the compiler.
    pub file: String,
    /// Demangled function name.
    pub function: String,
    /// First line of the function's regions that ran.
    pub start_line: u32,
    /// Last line of the function's regions that ran.
    pub end_line: u32,
    /// Failing runs that executed the function's most suspicious region.
    pub ef: u64,
    /// Passing runs that executed it.
    pub ep: u64,
    /// Failing runs that did not.
    pub nf: u64,
    /// Passing runs that did not.
    pub np: u64,
    /// The strongest scores among the function's regions.
    pub scores: Scores,
    /// Whether the function's lines contain the panic site.
    pub panic_overlap: bool,
    /// How much to trust the rank.
    pub confidence: Confidence,
    /// Which rule assigned the confidence.
    pub rule: &'static str,
}

/// The localization result attached to a failing report.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "json", derive(serde::Serialize))]
pub struct Locate {
    /// Passing runs in the spectrum.
    pub passing: u64,
    /// Failing runs in the spectrum (with the reported failure's signature).
    pub failing: u64,
    /// Regions executed by at least one run.
    pub regions: usize,
    /// The top suspects, most suspicious first.
    pub suspects: Vec<Suspect>,
}

struct Entry {
    /// Index into [`Functions::all`].
    function: usize,
    region: usize,
    /// The false arm of a branch region.
    false_arm: bool,
    ef: u64,
    ep: u64,
    failing_count: u64,
    passing_count: u64,
}

struct Group {
    counters: Range<usize>,
    entries: Range<usize>,
}

/// The pass/fail spectrum of one run.
pub(crate) struct Spectrum {
    mapping: &'static Mapping,
    entries: Vec<Entry>,
    /// Per tracked function, its counter window and its entries; sorted by
    /// counter start.
    groups: Vec<Group>,
    passing: u64,
    failing: u64,
}

impl Spectrum {
    /// Columns for the regions of `selected` (indices into
    /// [`Functions::all`]); `None` when the binary has no mapping.
    pub fn new(selected: &[usize]) -> Option<Spectrum> {
        let mapping = Mapping::get()?;
        let all = Functions::get().all();
        let mut entries = Vec::new();
        let mut groups = Vec::new();
        for &index in selected {
            let function = &all[index];
            let Some(mapped) = mapping.function(function.name_ref, function.func_hash) else {
                continue;
            };
            let start = entries.len();
            for (region, r) in mapped.regions.iter().enumerate() {
                match r.kind {
                    Kind::Code => entries.push(Entry::new(index, region, false)),
                    Kind::Branch => {
                        entries.push(Entry::new(index, region, false));
                        entries.push(Entry::new(index, region, true));
                    },
                    _ => {},
                }
            }
            if entries.len() > start {
                groups.push(Group {
                    counters: function.counters.clone(),
                    entries: start..entries.len(),
                });
            }
        }
        groups.sort_by_key(|g| g.counters.start);
        Some(Spectrum { mapping, entries, groups, passing: 0, failing: 0 })
    }

    /// Add one run's coverage as a passing or failing row.
    pub fn observe(&mut self, delta: &Delta, failing: bool) {
        if failing {
            self.failing += 1;
        } else {
            self.passing += 1;
        }
        let hits = delta.hits();
        let all = Functions::get().all();
        let mut values = Vec::new();
        for group in &self.groups {
            let first = hits.partition_point(|(i, _)| *i < group.counters.start);
            if first >= hits.len() || hits[first].0 >= group.counters.end {
                continue;
            }
            values.clear();
            values.resize(group.counters.len(), 0);
            for &(index, count) in &hits[first..] {
                if index >= group.counters.end {
                    break;
                }
                values[index - group.counters.start] = count;
            }
            let function: &Function = &all[self.entries[group.entries.start].function];
            let Some(mapped) = self.mapping.function(function.name_ref, function.func_hash) else {
                continue;
            };
            for entry in &mut self.entries[group.entries.clone()] {
                let region = &mapped.regions[entry.region];
                let counter = if entry.false_arm { region.false_counter } else { region.counter };
                let count = mapped.eval(counter, &values);
                if count == 0 {
                    continue;
                }
                if failing {
                    entry.ef += 1;
                    entry.failing_count += count;
                } else {
                    entry.ep += 1;
                    entry.passing_count += count;
                }
            }
        }
    }

    /// Score and rank; `panic` is the panic site (file, line) when known.
    pub fn rank(&self, panic: Option<(&str, u32)>, top: usize) -> Locate {
        let all = Functions::get().all();
        struct Scored<'a> {
            file: &'a str,
            function: &'a str,
            start_line: u32,
            end_line: u32,
            ef: u64,
            ep: u64,
            nf: u64,
            np: u64,
            scores: Scores,
        }
        let mut regions = Vec::new();
        for entry in &self.entries {
            if entry.ef + entry.ep == 0 {
                continue;
            }
            let function = &all[entry.function];
            let Some(mapped) = self.mapping.function(function.name_ref, function.func_hash) else {
                continue;
            };
            let region = &mapped.regions[entry.region];
            let nf = self.failing.saturating_sub(entry.ef);
            let np = self.passing.saturating_sub(entry.ep);
            let failing_avg = entry.failing_count as f64 / (self.failing as f64).max(1.0);
            let passing_avg = entry.passing_count as f64 / (self.passing as f64).max(1.0);
            let scores = Scores::compute(entry.ef, entry.ep, nf, np, failing_avg - passing_avg);
            regions.push(Scored {
                file: &self.mapping.files[region.file],
                function: &function.name,
                start_line: region.start_line,
                end_line: region.end_line,
                ef: entry.ef,
                ep: entry.ep,
                nf,
                np,
                scores,
            });
        }
        let executed = regions.len();

        // Merge the regions of a function: widest lines, strongest scores.
        let mut groups: HashMap<(&str, &str), (Scored<'_>, bool)> = HashMap::new();
        for r in regions {
            let overlaps = panic.is_some_and(|(file, line)| {
                same_file(r.file, file) && r.start_line <= line && line <= r.end_line
            });
            match groups.get_mut(&(r.file, r.function)) {
                Some((g, panic_overlap)) => {
                    g.start_line = g.start_line.min(r.start_line);
                    g.end_line = g.end_line.max(r.end_line);
                    if r.scores.ochiai > g.scores.ochiai {
                        (g.ef, g.ep, g.nf, g.np) = (r.ef, r.ep, r.nf, r.np);
                    }
                    let s = &mut g.scores;
                    s.ochiai = s.ochiai.max(r.scores.ochiai);
                    s.tarantula = s.tarantula.max(r.scores.tarantula);
                    s.dstar = s.dstar.max(r.scores.dstar);
                    s.jaccard = s.jaccard.max(r.scores.jaccard);
                    s.op2 = s.op2.max(r.scores.op2);
                    s.delta = s.delta.max(r.scores.delta);
                    *panic_overlap |= overlaps;
                },
                None => {
                    groups.insert((r.file, r.function), (r, overlaps));
                },
            }
        }
        let mut staged: Vec<(Scored<'_>, bool, f64)> =
            groups.into_values().map(|(g, p)| (g, p, 0.0)).collect();
        let max_ochiai = staged.iter().map(|s| s.0.scores.ochiai).fold(0.0f64, f64::max);
        let max_delta = staged.iter().map(|s| s.0.scores.delta).fold(0.0f64, f64::max);
        for (g, _, avg) in &mut staged {
            let o = if max_ochiai > 0.0 { g.scores.ochiai / max_ochiai } else { 0.0 };
            let d = if max_delta > 0.0 { g.scores.delta.max(0.0) / max_delta } else { 0.0 };
            *avg = (o + d) / 2.0;
        }
        staged.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal))
                .then_with(|| a.0.file.cmp(b.0.file))
                .then_with(|| a.0.start_line.cmp(&b.0.start_line))
                .then_with(|| a.0.function.cmp(b.0.function))
        });
        let ochiai_rank = rank_by(&staged, |s| s.0.scores.ochiai);
        let delta_rank = rank_by(&staged, |s| s.0.scores.delta);
        let panic_groups = staged.iter().filter(|s| s.1).count();

        let suspects = staged
            .iter()
            .enumerate()
            .take(top)
            .map(|(idx, (g, panic_overlap, _))| {
                let signal = g.scores.ochiai > 0.0 || g.scores.delta > 0.0;
                let (o1, d1) = (ochiai_rank[idx] == 1, delta_rank[idx] == 1);
                let (confidence, rule) = match (*panic_overlap, panic_groups, o1, d1) {
                    (true, 1, _, _) => (Confidence::High, "singleton_panic_overlap"),
                    (_, _, true, true) if signal => (Confidence::High, "both_metrics_rank_1"),
                    (_, _, true, false) | (_, _, false, true) if signal => {
                        (Confidence::Medium, "one_metric_rank_1")
                    },
                    (true, _, _, _) => (Confidence::MediumLow, "panic_multi_tier"),
                    _ if signal && (ochiai_rank[idx] as i64 - delta_rank[idx] as i64).abs() > 5 => {
                        (Confidence::Low, "metrics_disagree_gt5")
                    },
                    _ => (Confidence::LowMedium, "default"),
                };
                Suspect {
                    rank: idx + 1,
                    file: g.file.to_string(),
                    function: g.function.to_string(),
                    start_line: g.start_line,
                    end_line: g.end_line,
                    ef: g.ef,
                    ep: g.ep,
                    nf: g.nf,
                    np: g.np,
                    scores: g.scores,
                    panic_overlap: *panic_overlap,
                    confidence,
                    rule,
                }
            })
            .collect();
        Locate { passing: self.passing, failing: self.failing, regions: executed, suspects }
    }
}

impl Entry {
    fn new(function: usize, region: usize, false_arm: bool) -> Entry {
        Entry { function, region, false_arm, ef: 0, ep: 0, failing_count: 0, passing_count: 0 }
    }
}

/// 1-based rank of each element under `key`, descending.
fn rank_by<T>(items: &[T], key: impl Fn(&T) -> f64) -> Vec<usize> {
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|&a, &b| {
        key(&items[b]).partial_cmp(&key(&items[a])).unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut ranks = vec![0; items.len()];
    for (rank, index) in order.into_iter().enumerate() {
        ranks[index] = rank + 1;
    }
    ranks
}

/// Whether a mapping file name and a panic location name the same file:
/// the compiler may record one absolute and the other relative.
fn same_file(mapped: &str, panic: &str) -> bool {
    let (m, p) = (mapped.replace('\\', "/"), panic.replace('\\', "/"));
    m == p || m.ends_with(&format!("/{p}")) || p.ends_with(&format!("/{m}"))
}

/// The source lines an input executed, per file, from its counter delta and
/// the mapping of the selected functions; `None` without a mapping. Code
/// regions with a non-zero count contribute every line they span.
pub(crate) fn lines_hit(delta: &Delta, selected: &[usize]) -> Option<BTreeMap<String, Vec<u32>>> {
    let mapping = Mapping::get()?;
    let all = Functions::get().all();
    let hits = delta.hits();
    let mut lines: BTreeMap<usize, BTreeSet<u32>> = BTreeMap::new();
    let mut values = Vec::new();
    for &index in selected {
        let function = &all[index];
        let range = &function.counters;
        let first = hits.partition_point(|(i, _)| *i < range.start);
        if first >= hits.len() || hits[first].0 >= range.end {
            continue;
        }
        let Some(mapped) = mapping.function(function.name_ref, function.func_hash) else {
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
        for region in &mapped.regions {
            let count = match region.kind {
                Kind::Code => mapped.eval(region.counter, &values),
                Kind::Branch => {
                    mapped.eval(region.counter, &values)
                        + mapped.eval(region.false_counter, &values)
                },
                _ => 0,
            };
            if count > 0 {
                lines.entry(region.file).or_default().extend(region.start_line..=region.end_line);
            }
        }
    }
    Some(
        lines
            .into_iter()
            .map(|(file, set)| (mapping.files[file].clone(), set.into_iter().collect()))
            .collect(),
    )
}

/// Shorten a file name for display: relative to the current directory when
/// it lies inside it.
pub(crate) fn display_path(file: &str) -> String {
    if let Ok(cwd) = std::env::current_dir()
        && let Some(rest) = file.strip_prefix(&*cwd.to_string_lossy())
    {
        return rest.trim_start_matches(['/', '\\']).to_string();
    }
    file.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores_match_the_reference_formulas() {
        let s = Scores::compute(3, 1, 0, 5, 0.5);
        assert!((s.ochiai - 3.0 / (3.0f64 * 4.0).sqrt()).abs() < 1e-12);
        assert!((s.tarantula - (1.0 / (1.0 + 1.0 / 6.0))).abs() < 1e-12);
        assert!((s.dstar - 9.0).abs() < 1e-12);
        assert!((s.jaccard - 0.75).abs() < 1e-12);
        assert!((s.op2 - (3.0 - 1.0 / 7.0)).abs() < 1e-12);
        assert_eq!(Scores::compute(0, 0, 2, 2, 0.0), Scores::default());
        assert_eq!(Scores::compute(2, 0, 0, 0, 0.0).dstar, f64::MAX);
    }

    #[test]
    fn ranks_descending_with_ties_in_order() {
        assert_eq!(rank_by(&[0.5, 0.9, 0.1], |x| *x), [2, 1, 3]);
    }

    #[test]
    fn file_names_match_by_suffix() {
        assert!(same_file("/work/src/lib.rs", "src/lib.rs"));
        assert!(same_file("src/lib.rs", "/work/src/lib.rs"));
        assert!(!same_file("/work/src/lib.rs", "lib.rs.bak"));
        assert!(!same_file("/work/src/mylib.rs", "lib.rs"));
    }
}
