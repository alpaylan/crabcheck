//! Per-run coverage bookkeeping: which counters each input touched, what
//! was new, and the summary that goes into the report.

use std::{
    collections::BTreeSet,
    ops::Range,
    path::Path,
};

use super::counters::{
    Functions,
    Snapshot,
    snapshot_range,
};

/// AFL-style hit-count classes: a counter that went from "hit 3 times" to
/// "hit 30 times" in one input counts as new coverage once.
fn bucket(count: u64) -> u8 {
    match count {
        0 => 0,
        1 => 1,
        2 => 2,
        3 => 3,
        4..=7 => 4,
        8..=15 => 5,
        16..=31 => 6,
        32..=127 => 7,
        _ => 8,
    }
}

/// Tracks coverage over the inputs of one run.
pub(crate) struct Tracker {
    /// Counter range covering the selected functions.
    window: Range<usize>,
    /// Indices into [`Functions::all`] of the selected functions.
    selected: Vec<usize>,
    /// Highest hit-count bucket seen per counter in the window.
    virgin: Vec<u8>,
    /// How many inputs touched each counter in the window.
    touched_by: Vec<u32>,
    /// Inputs observed.
    pub inputs: u64,
    /// Inputs that revealed new coverage.
    pub novel_inputs: u64,
    /// The input index (0-based, over observed inputs) at which coverage last grew.
    pub last_novelty_at: u64,
    /// Name prefix used to select functions, for the report.
    filter: String,
}

/// What one input did.
pub(crate) struct Observation {
    /// Whether the input reached a counter, or a hit-count class, never
    /// seen before in this run.
    pub novel: bool,
}

impl Tracker {
    /// Track the functions whose demangled names start with `filter`
    /// (`crate` or `crate::module`), or every function except crabcheck's
    /// own when `filter` is empty. `None` when the binary is uninstrumented
    /// or nothing matches.
    pub fn new(filter: &str) -> Option<Tracker> {
        let functions = Functions::get();
        let selected: Vec<usize> = functions
            .all()
            .iter()
            .enumerate()
            .filter(
                |(_, f)| {
                    if filter.is_empty() { !is_own(&f.name) } else { matches(&f.name, filter) }
                },
            )
            .map(|(i, _)| i)
            .collect();
        if selected.is_empty() {
            return None;
        }
        let lo = selected.iter().map(|&i| functions.all()[i].counters.start).min()?;
        let hi = selected.iter().map(|&i| functions.all()[i].counters.end).max()?;
        let width = hi - lo;
        Some(Tracker {
            window: lo..hi,
            selected,
            virgin: vec![0; width],
            touched_by: vec![0; width],
            inputs: 0,
            novel_inputs: 0,
            last_novelty_at: 0,
            filter: filter.to_string(),
        })
    }

    /// Snapshot the window before running an input.
    pub fn begin(&self) -> Option<Snapshot> {
        snapshot_range(&self.window)
    }

    /// Record what the input did since `before`.
    pub fn end(&mut self, before: &Snapshot) -> Observation {
        let after = snapshot_range(&self.window).unwrap_or_else(|| before.clone());
        let delta = before.delta(&after);
        let mut novel = false;
        for &(index, count) in delta.hits() {
            let slot = index - self.window.start;
            self.touched_by[slot] += 1;
            let class = bucket(count);
            if class > self.virgin[slot] {
                self.virgin[slot] = class;
                novel = true;
            }
        }
        if novel {
            self.novel_inputs += 1;
            self.last_novelty_at = self.inputs;
        }
        self.inputs += 1;
        Observation { novel }
    }

    /// Summary for the report.
    pub fn summary(&self) -> Summary {
        let all = Functions::get().all();
        let mut reached = BTreeSet::new();
        let mut unreached = BTreeSet::new();
        let mut partial = Vec::new();
        for &i in &self.selected {
            let f = &all[i];
            let hit =
                f.counters.clone().filter(|c| self.touched_by[c - self.window.start] > 0).count();
            if hit == 0 {
                unreached.insert(f.name.clone());
            } else {
                reached.insert(f.name.clone());
                if hit < f.counters.len() {
                    partial.push((f.name.clone(), hit, f.counters.len()));
                }
            }
        }
        partial.sort_by(|a, b| (b.2 - b.1).cmp(&(a.2 - a.1)).then_with(|| a.0.cmp(&b.0)));
        let counters_hit = self.touched_by.iter().filter(|c| **c > 0).count();
        let counters_total = self.selected.iter().map(|&i| all[i].counters.len()).sum();
        Summary {
            filter: self.filter.clone(),
            functions_reached: reached.len(),
            functions_total: self.selected.len(),
            counters_hit,
            counters_total,
            inputs: self.inputs,
            novel_inputs: self.novel_inputs,
            last_novelty_at: self.last_novelty_at,
            unreached: unreached.into_iter().collect(),
            partial,
        }
    }
}

/// Coverage of one run, as reported.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
pub struct Summary {
    /// The function-name prefix that was tracked (empty: everything but crabcheck).
    pub filter: String,
    /// Selected functions reached by at least one input.
    pub functions_reached: usize,
    /// Selected functions in the binary.
    pub functions_total: usize,
    /// Counters (regions) hit by at least one input.
    pub counters_hit: usize,
    /// Counters (regions) in the selected functions.
    pub counters_total: usize,
    /// Inputs observed.
    pub inputs: u64,
    /// Inputs that revealed new coverage.
    pub novel_inputs: u64,
    /// The observed-input index at which coverage last grew.
    pub last_novelty_at: u64,
    /// Selected functions no input reached.
    pub unreached: Vec<String>,
    /// Reached functions with counters no input hit, as (name, hit, total),
    /// most missed first: the branches the generator does not exercise.
    pub partial: Vec<(String, usize, usize)>,
}

fn is_own(name: &str) -> bool {
    let bare = name.trim_start_matches('<');
    bare.starts_with("crabcheck::") || bare.starts_with("crabcheck_macros::")
}

fn matches(name: &str, prefix: &str) -> bool {
    let bare = name.trim_start_matches('<');
    bare == prefix || bare.starts_with(prefix) && bare[prefix.len()..].starts_with("::")
}

/// Write a profraw image of the process's counters to `<dir>/<name>.profraw`
/// for `llvm-cov`. The image covers everything that ran in the process so
/// far, not only this property.
pub(crate) fn write_profraw(dir: &Path, name: &str) -> std::io::Result<std::path::PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.profraw", crate::regressions_file_stem(name)));
    let image = super::counters::profraw_image()
        .ok_or_else(|| std::io::Error::other("coverage unavailable"))?;
    std::fs::write(&path, image)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_are_monotone_classes() {
        let classes: Vec<u8> = [0u64, 1, 2, 3, 4, 7, 8, 15, 16, 31, 32, 127, 128, 10_000]
            .iter()
            .map(|&c| bucket(c))
            .collect();
        assert_eq!(classes, vec![0, 1, 2, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8]);
    }

    #[test]
    fn name_matching() {
        assert!(matches("my_crate::a::b", "my_crate"));
        assert!(matches("<my_crate::T as Trait>::f", "my_crate"));
        assert!(!matches("my_crate_other::a", "my_crate"));
        assert!(is_own("crabcheck::runner::check"));
        assert!(!is_own("crabby::x"));
    }

    #[test]
    fn tracker_matches_the_build() {
        match Tracker::new("") {
            None => assert!(!super::super::available()),
            Some(mut tracker) => {
                let before = tracker.begin().unwrap();
                let observation = tracker.end(&before);
                assert_eq!(tracker.inputs, 1);
                let _ = observation;
                let summary = tracker.summary();
                assert_eq!(summary.functions_total, tracker.selected.len());
            },
        }
    }
}
