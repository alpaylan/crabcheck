//! The regressions file: failing inputs remembered across runs.
//!
//! When a property fails in the random phase, where the input came from is
//! appended to `<dir>/<name>.jsonl` (one JSON object per line: name, seed,
//! iteration, the input's `Debug` rendering, the shrunk rendering, creation
//! time). The next run of the same property regenerates every stored input
//! before random testing and checks it still renders the same; an entry
//! whose generator has changed is reported as stale rather than silently
//! testing a different input. Commit the directory to keep that guarantee
//! for the whole team.
//!
//! Requires the default `json` feature.

use std::{
    fs,
    io::{
        self,
        Write,
    },
    path::{
        Path,
        PathBuf,
    },
    time::{
        SystemTime,
        UNIX_EPOCH,
    },
};

/// One remembered failure.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    /// The property's name (see `Config::name`).
    pub name: String,
    /// Seed of the run that found it.
    pub seed: u64,
    /// Generation attempt within that run.
    pub iteration: u64,
    /// `Debug` rendering of the input as generated, used to detect that the
    /// generator has changed.
    pub debug: String,
    /// `Debug` rendering of the shrunk input, for humans reading the file.
    #[serde(default)]
    pub shrunk: Option<String>,
    /// Unix time (seconds) when the entry was written.
    pub created: u64,
}

impl Entry {
    pub(crate) fn new(
        name: &str,
        seed: u64,
        iteration: u64,
        debug: String,
        shrunk: Option<String>,
    ) -> Self {
        let created = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        Entry { name: name.to_string(), seed, iteration, debug, shrunk, created }
    }
}

/// Turn a property name (often `module::test_fn`) into a file name.
pub fn file_name(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' })
        .collect();
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    let trimmed = out.trim_matches('_');
    let base = if trimmed.is_empty() { "unnamed" } else { trimmed };
    format!("{base}.jsonl")
}

/// The file that holds entries for `name` under `dir`.
pub fn path_for(dir: &Path, name: &str) -> PathBuf {
    dir.join(file_name(name))
}

/// Load the entries stored for `name`. Missing files and unparsable lines
/// are skipped.
pub fn load(dir: &Path, name: &str) -> Vec<Entry> {
    let Ok(text) = fs::read_to_string(path_for(dir, name)) else {
        return Vec::new();
    };
    text.lines().filter_map(|line| serde_json::from_str::<Entry>(line).ok()).collect()
}

/// Append `entry` unless an entry for the same seed and iteration is
/// already stored. Returns the file's path and whether a line was appended.
pub fn save(dir: &Path, entry: &Entry) -> io::Result<(PathBuf, bool)> {
    let path = path_for(dir, &entry.name);
    if load(dir, &entry.name).iter().any(|e| e.seed == entry.seed && e.iteration == entry.iteration)
    {
        return Ok((path, false));
    }
    fs::create_dir_all(dir)?;
    let mut file = fs::OpenOptions::new().create(true).append(true).open(&path)?;
    let line = serde_json::to_string(entry).map_err(io::Error::other)?;
    writeln!(file, "{line}")?;
    Ok((path, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("crabcheck-regressions-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn file_names_are_sanitised() {
        assert_eq!(
            file_name("runner::tests::passing_property"),
            "runner_tests_passing_property.jsonl"
        );
        assert_eq!(file_name("  "), "unnamed.jsonl");
        assert_eq!(file_name("a/b c"), "a_b_c.jsonl");
    }

    #[test]
    fn save_then_load_dedupes_by_point() {
        let dir = temp_dir("dedupe");
        assert!(load(&dir, "p").is_empty());
        let entry = Entry::new("p", 7, 3, "[1]".into(), Some("[]".into()));
        let (path, appended) = save(&dir, &entry).unwrap();
        assert_eq!(path, dir.join("p.jsonl"));
        assert!(appended);
        assert!(!save(&dir, &entry).unwrap().1);
        save(&dir, &Entry::new("p", 8, 0, "[2]".into(), None)).unwrap();
        let loaded = load(&dir, "p");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].iteration, 3);
        assert_eq!(loaded[1].seed, 8);
        let _ = fs::remove_dir_all(&dir);
    }
}
