//! The coverage grid: a self-contained HTML page with every tracked
//! function's lines, how many inputs executed each, the targets and the
//! sampled inputs, from which a line range can be turned into a new target
//! and a brief for the agent that will rewrite the generator.

use std::{
    collections::BTreeMap,
    path::{
        Path,
        PathBuf,
    },
};

use super::{
    counters::{
        Delta,
        Functions,
    },
    mapping::Mapping,
    targets::TargetOutcome,
};

/// What the page says about the run it shows.
pub(crate) struct Facts<'a> {
    pub property: &'a str,
    pub seed: u64,
    pub tests: u64,
    pub filter: &'a str,
    /// `type_name` of the property, used to point at its function.
    pub property_type: &'a str,
}

/// Per-line accumulation over a run for the selected functions.
pub(crate) struct Grid {
    selected: Vec<usize>,
    /// Inputs that entered each selected function.
    entered: BTreeMap<usize, u64>,
    /// (function, file, line) -> (inputs that executed it, executions).
    lines: BTreeMap<(usize, usize, u32), (u64, u64)>,
    inputs: u64,
}

impl Grid {
    /// Start accumulating for `selected` (indices into [`Functions::all`]);
    /// `None` without a mapping.
    pub fn new(selected: &[usize]) -> Option<Grid> {
        Mapping::get()?;
        Some(Grid {
            selected: selected.to_vec(),
            entered: BTreeMap::new(),
            lines: BTreeMap::new(),
            inputs: 0,
        })
    }

    /// Fold one input's counter delta in.
    pub fn observe(&mut self, delta: &Delta) {
        self.inputs += 1;
        let Some(mapping) = Mapping::get() else {
            return;
        };
        let all = Functions::get().all();
        let hits = delta.hits();
        let mut values = Vec::new();
        for &fi in &self.selected {
            let range = &all[fi].counters;
            let first = hits.partition_point(|(i, _)| *i < range.start);
            if first >= hits.len() || hits[first].0 >= range.end {
                continue;
            }
            let Some(mapped) = mapping.function(all[fi].name_ref, all[fi].func_hash) else {
                continue;
            };
            *self.entered.entry(fi).or_default() += 1;
            values.clear();
            values.resize(range.len(), 0);
            for &(i, count) in &hits[first..] {
                if i >= range.end {
                    break;
                }
                values[i - range.start] = count;
            }
            for ((file, line), count) in mapped.line_counts(&values) {
                if count > 0 {
                    let entry = self.lines.entry((fi, file, line)).or_default();
                    entry.0 += 1;
                    entry.1 += count;
                }
            }
        }
    }

    /// Render the page. `samples` are `(debug, origin, size)`.
    pub fn render(
        &self,
        facts: &Facts<'_>,
        targets: &[TargetOutcome],
        samples: &[(String, String, usize)],
    ) -> String {
        use serde_json::json;
        let mapping = Mapping::get();
        let all = Functions::get().all();
        let property_fn = self
            .selected
            .iter()
            .filter(|&&fi| super::is_property_fn(&all[fi].name, facts.property_type))
            .max_by_key(|&&fi| {
                (
                    all[fi].name.ends_with("::__crabcheck_property"),
                    self.entered.get(&fi).copied().unwrap_or(0),
                )
            })
            .map(|&fi| all[fi].name.clone());
        let mut files: BTreeMap<usize, Vec<serde_json::Value>> = BTreeMap::new();
        let mut reached = 0usize;
        if let Some(mapping) = mapping {
            for &fi in &self.selected {
                let function = &all[fi];
                let Some(mapped) = mapping.function(function.name_ref, function.func_hash) else {
                    continue;
                };
                let zero = vec![0u64; function.counters.len()];
                // The lines the function owns, grouped by file (macro
                // expansions can put a few elsewhere).
                let mut per_file: BTreeMap<usize, Vec<serde_json::Value>> = BTreeMap::new();
                for ((file, line), _) in mapped.line_counts(&zero) {
                    let (inputs, count) =
                        self.lines.get(&(fi, file, line)).copied().unwrap_or((0, 0));
                    per_file.entry(file).or_default().push(json!([line, inputs, count]));
                }
                let entered = self.entered.get(&fi).copied().unwrap_or(0);
                if entered > 0 {
                    reached += 1;
                }
                for (file, lines) in per_file {
                    let start = lines.first().and_then(|l| l[0].as_u64()).unwrap_or(0);
                    let end = lines.last().and_then(|l| l[0].as_u64()).unwrap_or(0);
                    files.entry(file).or_default().push(json!({
                        "name": function.name,
                        "module": module_of(&function.name),
                        "start": start,
                        "end": end,
                        "inputs": entered,
                        "lines": lines,
                    }));
                }
            }
        }
        let file_index: BTreeMap<usize, usize> =
            files.keys().enumerate().map(|(i, &f)| (f, i)).collect();
        let files_json: Vec<serde_json::Value> = files
            .iter()
            .map(|(&file, functions)| {
                let path = mapping.map_or("", |m| m.files[file].as_str());
                json!({
                    "path": path,
                    "display": super::display_path(path),
                    "source": source_lines(path),
                    "functions": functions,
                })
            })
            .collect();
        let targets_json: Vec<serde_json::Value> = targets
            .iter()
            .map(|t| {
                let file_idx = mapping.and_then(|m| {
                    m.files
                        .iter()
                        .enumerate()
                        .find(|(_, f)| super::locate::same_file(f, &t.target.file))
                        .and_then(|(i, _)| file_index.get(&i).copied())
                });
                json!({
                    "file": t.target.file,
                    "begin_line": t.target.begin_line,
                    "end_line": t.target.end_line,
                    "file_index": file_idx,
                    "functions": t.functions,
                    "hits": t.hits,
                    "first_hit_at": t.first_hit_at,
                    "function_inputs": t.function_inputs,
                })
            })
            .collect();
        let data = json!({
            "property": facts.property,
            "seed": facts.seed,
            "tests": facts.tests,
            "inputs": self.inputs,
            "filter": facts.filter,
            "functions_reached": reached,
            "property_fn": property_fn,
            "generated": timestamp(),
            "files": files_json,
            "targets": targets_json,
            "samples": samples.iter().map(|(debug, origin, size)| json!({"debug": debug, "origin": origin, "size": size})).collect::<Vec<_>>(),
        });
        let payload = data.to_string().replace("</", "<\\/");
        include_str!("grid.html").replacen("__CRABCHECK_DATA__", &payload, 1).replacen(
            "<title>crabcheck coverage</title>",
            &format!("<title>crabcheck coverage: {}</title>", escape(facts.property)),
            1,
        )
    }

    /// Write the page to `<dir>/<name>.html`.
    pub fn write(&self, dir: &Path, name: &str, html: &str) -> std::io::Result<PathBuf> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!("{}.html", file_stem(name)));
        std::fs::write(&path, html)?;
        Ok(path)
    }
}

/// The path of a function name: everything before its last `::`.
fn module_of(name: &str) -> String {
    match name.rfind("::") {
        Some(i) => name[..i].to_string(),
        None => name.to_string(),
    }
}

/// Source lines of a file when it can be read and is not huge.
fn source_lines(path: &str) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(path).ok()?;
    if text.len() > 2_000_000 {
        return None;
    }
    Some(text.lines().map(|l| l.to_string()).collect())
}

fn timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    crate::report::civil_time(secs)
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn file_stem(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' })
        .collect();
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() { "unnamed".to_string() } else { trimmed.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modules_and_stems() {
        assert_eq!(module_of("bst::main::{closure#1}"), "bst::main");
        assert_eq!(module_of("<bst::Tree>::insert"), "<bst::Tree>");
        assert_eq!(module_of("main"), "main");
        assert_eq!(file_stem("runner::tests::x"), "runner_tests_x");
        assert_eq!(file_stem("  "), "unnamed");
    }

    #[test]
    fn timestamps_are_civil_dates() {
        let stamp = timestamp();
        assert_eq!(stamp.len(), "2026-09-06 21:18 UTC".len(), "{stamp}");
        assert!(stamp.starts_with("20"), "{stamp}");
    }
}
