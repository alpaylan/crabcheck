//! Run configuration.
//!
//! All fields are public so a test can use struct-update syntax, and every
//! scalar field has a chainable setter of the same name:
//!
//! ```
//! use crabcheck::Config;
//!
//! let a = Config { tests: 1000, seed: Some(7), ..Config::default() };
//! let b = Config::default().tests(1000).seed(7);
//! assert_eq!(a, b);
//! ```
//!
//! # Environment overrides
//!
//! [`check`](crate::check) applies overrides from the environment on top of
//! the configuration it is given, so a run can be steered without editing
//! code. Precedence: code < `CRABCHECK_CONFIG` file < `CRABCHECK_*`
//! variables. Problems (unknown `CRABCHECK_*` names, unparsable values) are
//! reported as `config_env` health warnings, never silently ignored.
//!
//! | Variable | Field |
//! |---|---|
//! | `CRABCHECK_CONFIG` | path of a JSON file with any subset of fields (needs the `json` feature) |
//! | `CRABCHECK_TESTS` | `tests` |
//! | `CRABCHECK_SEED` | `seed` |
//! | `CRABCHECK_MAX_SIZE` | `max_size` |
//! | `CRABCHECK_MAX_DISCARD_RATIO` | `max_discard_ratio` |
//! | `CRABCHECK_BUDGET_MS` | `budget_ms` |
//! | `CRABCHECK_TIMEOUT_MS` | `timeout_ms` |
//! | `CRABCHECK_FILTER_MAX_ATTEMPTS` | `filter_max_attempts` |
//! | `CRABCHECK_THREADS` | `threads` (for `check_par`) |
//! | `CRABCHECK_NAME` | `name` |
//! | `CRABCHECK_REPLAY` | `replay` (`seed:iteration` from a report) |
//! | `CRABCHECK_SHRINK` | `shrink.enabled` (`0`/`1`, `false`/`true`) |
//! | `CRABCHECK_SHRINK_MAX_STEPS` | `shrink.max_steps` |
//! | `CRABCHECK_SHRINK_TIMEOUT_MS` | `shrink.timeout_ms` |
//! | `CRABCHECK_TARGET_TESTS` | `target.tests` |
//! | `CRABCHECK_TARGET_POOL_SIZE` | `target.pool_size` |
//! | `CRABCHECK_REGRESSIONS` | `regressions.save` and `regressions.replay` |
//! | `CRABCHECK_REGRESSIONS_DIR` | `regressions.dir` |
//! | `CRABCHECK_TRUNCATE_DEBUG_AT` | `report.truncate_debug_at` |
//! | `CRABCHECK_REPORT_JSON` | `report.json_path` |

use std::{
    fmt,
    path::PathBuf,
    str::FromStr,
    time::Duration,
};

/// Where an input can be regenerated from: the run seed and the generation
/// attempt within the run. Printed as `seed:iteration` in reports and
/// accepted by `CRABCHECK_REPLAY`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
pub struct ReplayPoint {
    /// The run seed.
    pub seed: u64,
    /// Zero-based generation attempt.
    pub iteration: u64,
}

impl fmt::Display for ReplayPoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.seed, self.iteration)
    }
}

impl FromStr for ReplayPoint {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (seed, iteration) =
            s.trim().split_once(':').ok_or_else(|| "expected seed:iteration".to_string())?;
        Ok(ReplayPoint {
            seed: seed.trim().parse().map_err(|e| format!("bad seed: {e}"))?,
            iteration: iteration.trim().parse().map_err(|e| format!("bad iteration: {e}"))?,
        })
    }
}

/// Everything that controls a run of [`check`](crate::check).
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize), serde(default))]
pub struct Config {
    /// Number of passing tests required before the property is considered
    /// to hold. Discarded inputs do not count. Default 10 000.
    pub tests: u64,
    /// Stop the random phase after this many milliseconds even if fewer than
    /// `tests` inputs have passed; the report says so. Default none.
    pub budget_ms: Option<u64>,
    /// Seed for the run. `None` picks a fresh random seed; the seed actually
    /// used is always in the report.
    pub seed: Option<u64>,
    /// Upper bound of the size parameter generators see. Each input draws
    /// its own size: exactly `max_size` one time in sixteen, otherwise
    /// `floor(max_size * u^2)` for `u` uniform in `[0, 1)`, so small inputs
    /// are common and the maximum is reached regularly. Default 100.
    pub max_size: usize,
    /// Give up once more than `max_discard_ratio * tests` inputs have been
    /// discarded. Default 10.
    pub max_discard_ratio: f64,
    /// Stop with `TimedOut` after this many milliseconds. Default none.
    pub timeout_ms: Option<u64>,
    /// Attempts a `filter` makes before discarding the input. Default 100.
    pub filter_max_attempts: usize,
    /// Worker threads for [`check_par`](crate::check_par); `0` means one per
    /// available CPU. Ignored by [`check`](crate::check). Default 1.
    pub threads: usize,
    /// Name of the property, used as the regressions file name. Defaults to
    /// the current thread's name, which under `cargo test` is the test's
    /// path (`module::test_fn`).
    pub name: Option<String>,
    /// An input to regenerate and test first (see `CRABCHECK_REPLAY`). The
    /// run continues with random testing if it does not fail.
    pub replay: Option<ReplayPoint>,
    /// Shrinking budget.
    pub shrink: ShrinkConfig,
    /// The targeted phase (used when the property calls [`target`](crate::target)).
    pub target: TargetConfig,
    /// Remembered failures.
    pub regressions: RegressionsConfig,
    /// Report rendering.
    pub report: ReportConfig,
}

/// Budget for shrinking a counterexample.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize), serde(default))]
pub struct ShrinkConfig {
    /// Whether to shrink at all. Default true.
    pub enabled: bool,
    /// Maximum number of shrink candidates to try. Default 10 000.
    pub max_steps: u64,
    /// Time budget for shrinking in milliseconds. Default 60 000.
    pub timeout_ms: Option<u64>,
}

/// Budget of the targeted phase, which runs after the random phase when
/// the property reported scores through [`target`](crate::target).
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize), serde(default))]
pub struct TargetConfig {
    /// Mutated inputs to try. `0` disables the targeted phase. Default 256.
    pub tests: u64,
    /// Best-scoring inputs kept as mutation seeds. Default 8.
    pub pool_size: usize,
    /// Mutations tried per seed before it is retired. Default 16.
    pub energy: u32,
    /// Share of iterations that draw a fresh random input instead of
    /// mutating a seed. Default 0.1.
    pub explore: f64,
}

/// Where and whether failing inputs are remembered (needs the `json`
/// feature; without it both flags are ignored).
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize), serde(default))]
pub struct RegressionsConfig {
    /// Directory holding one `<name>.jsonl` per property. Default
    /// `.crabcheck/regressions`, relative to the working directory (the
    /// crate root under `cargo test`).
    pub dir: PathBuf,
    /// Append the shrunk failing input on failure. Default true.
    pub save: bool,
    /// Replay stored entries before random testing. Default true.
    pub replay: bool,
}

/// How reports are rendered.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize), serde(default))]
pub struct ReportConfig {
    /// `Debug` renderings of counterexamples longer than this many characters
    /// are truncated (the report says so). Default 2 000.
    pub truncate_debug_at: usize,
    /// Append each report as one JSON line to this file (needs the `json`
    /// feature). Default none.
    pub json_path: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            tests: 10_000,
            budget_ms: None,
            seed: None,
            max_size: 100,
            max_discard_ratio: 10.0,
            timeout_ms: None,
            filter_max_attempts: crate::source::DEFAULT_FILTER_MAX_ATTEMPTS,
            threads: 1,
            name: None,
            replay: None,
            shrink: ShrinkConfig::default(),
            target: TargetConfig::default(),
            regressions: RegressionsConfig::default(),
            report: ReportConfig::default(),
        }
    }
}

impl Default for ShrinkConfig {
    fn default() -> Self {
        ShrinkConfig { enabled: true, max_steps: 10_000, timeout_ms: Some(60_000) }
    }
}

impl Default for TargetConfig {
    fn default() -> Self {
        TargetConfig { tests: 256, pool_size: 8, energy: 16, explore: 0.1 }
    }
}

impl Default for RegressionsConfig {
    fn default() -> Self {
        RegressionsConfig { dir: PathBuf::from(".crabcheck/regressions"), save: true, replay: true }
    }
}

impl Default for ReportConfig {
    fn default() -> Self {
        ReportConfig { truncate_debug_at: 2_000, json_path: None }
    }
}

impl Config {
    /// The default configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// The default configuration with environment overrides applied (see
    /// the [module docs](self)). Problems are dropped; use [`check`](crate::check),
    /// which reports them, when that matters.
    pub fn from_env() -> Self {
        Self::default().with_env()
    }

    /// Apply environment overrides on top of this configuration.
    pub fn with_env(self) -> Self {
        self.apply_env_vars(std::env::vars()).0
    }

    /// Set [`tests`](Self::tests).
    pub fn tests(mut self, tests: u64) -> Self {
        self.tests = tests;
        self
    }

    /// Set [`budget_ms`](Self::budget_ms) from a `Duration`.
    pub fn budget(mut self, budget: Duration) -> Self {
        self.budget_ms = Some(budget.as_millis() as u64);
        self
    }

    /// Set [`seed`](Self::seed).
    pub fn seed(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }

    /// Set [`max_size`](Self::max_size).
    pub fn max_size(mut self, max_size: usize) -> Self {
        self.max_size = max_size;
        self
    }

    /// Set [`max_discard_ratio`](Self::max_discard_ratio).
    pub fn max_discard_ratio(mut self, ratio: f64) -> Self {
        self.max_discard_ratio = ratio;
        self
    }

    /// Set [`timeout_ms`](Self::timeout_ms) from a `Duration`.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout_ms = Some(timeout.as_millis() as u64);
        self
    }

    /// Set [`filter_max_attempts`](Self::filter_max_attempts).
    pub fn filter_max_attempts(mut self, attempts: usize) -> Self {
        self.filter_max_attempts = attempts.max(1);
        self
    }

    /// Set [`threads`](Self::threads).
    pub fn threads(mut self, threads: usize) -> Self {
        self.threads = threads;
        self
    }

    /// Set [`name`](Self::name).
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set [`replay`](Self::replay).
    pub fn replay(mut self, point: ReplayPoint) -> Self {
        self.replay = Some(point);
        self
    }

    /// Disable shrinking.
    pub fn no_shrink(mut self) -> Self {
        self.shrink.enabled = false;
        self
    }

    /// Neither save nor replay regressions.
    pub fn no_regressions(mut self) -> Self {
        self.regressions.save = false;
        self.regressions.replay = false;
        self
    }

    /// Set [`regressions.dir`](RegressionsConfig::dir).
    pub fn regressions_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.regressions.dir = dir.into();
        self
    }

    /// The run timeout as a `Duration`, if any.
    pub fn timeout_duration(&self) -> Option<Duration> {
        self.timeout_ms.map(Duration::from_millis)
    }

    /// Parse a configuration from JSON. Missing fields take their defaults.
    #[cfg(feature = "json")]
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Render the configuration as pretty-printed JSON.
    #[cfg(feature = "json")]
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("Config is always serialisable")
    }

    /// Apply `CRABCHECK_*` overrides from `vars`, returning the new
    /// configuration and a description of every problem encountered.
    pub fn apply_env_vars(
        mut self,
        vars: impl IntoIterator<Item = (String, String)>,
    ) -> (Self, Vec<String>) {
        let mut problems = Vec::new();
        let mut vars: Vec<(String, String)> =
            vars.into_iter().filter(|(k, _)| k.starts_with("CRABCHECK_")).collect();
        vars.sort();

        // The config file comes first so that individual variables win.
        if let Some((_, path)) = vars.iter().find(|(k, _)| k == "CRABCHECK_CONFIG") {
            #[cfg(feature = "json")]
            match std::fs::read_to_string(path) {
                Ok(text) => {
                    match Self::from_json(&text) {
                        Ok(from_file) => self = from_file,
                        Err(e) => {
                            problems.push(format!("CRABCHECK_CONFIG={path}: invalid JSON: {e}"))
                        },
                    }
                },
                Err(e) => problems.push(format!("CRABCHECK_CONFIG={path}: cannot read: {e}")),
            }
            #[cfg(not(feature = "json"))]
            problems.push(format!(
                "CRABCHECK_CONFIG={path}: ignored, this build has no `json` feature"
            ));
        }

        for (key, value) in &vars {
            let mut parse = |what: &str, apply: &mut dyn FnMut(&str) -> bool| {
                if !apply(value) {
                    problems.push(format!("{key}={value}: expected {what}, ignored"));
                }
            };
            match key.as_str() {
                "CRABCHECK_CONFIG" => {},
                "CRABCHECK_TESTS" => {
                    parse("an integer", &mut |v| v.parse().map(|n| self.tests = n).is_ok())
                },
                "CRABCHECK_SEED" => {
                    parse("an integer", &mut |v| v.parse().map(|n| self.seed = Some(n)).is_ok())
                },
                "CRABCHECK_MAX_SIZE" => {
                    parse("an integer", &mut |v| v.parse().map(|n| self.max_size = n).is_ok())
                },
                "CRABCHECK_MAX_DISCARD_RATIO" => {
                    parse("a number", &mut |v| {
                        v.parse().map(|n| self.max_discard_ratio = n).is_ok()
                    })
                },
                "CRABCHECK_BUDGET_MS" => {
                    parse("an integer", &mut |v| {
                        v.parse().map(|n| self.budget_ms = Some(n)).is_ok()
                    })
                },
                "CRABCHECK_TIMEOUT_MS" => {
                    parse("an integer", &mut |v| {
                        v.parse().map(|n| self.timeout_ms = Some(n)).is_ok()
                    })
                },
                "CRABCHECK_FILTER_MAX_ATTEMPTS" => {
                    parse("an integer", &mut |v| {
                        v.parse::<usize>().map(|n| self.filter_max_attempts = n.max(1)).is_ok()
                    })
                },
                "CRABCHECK_THREADS" => {
                    parse("an integer", &mut |v| v.parse().map(|n| self.threads = n).is_ok())
                },
                "CRABCHECK_NAME" => self.name = Some(value.clone()),
                "CRABCHECK_REPLAY" => {
                    parse("seed:iteration", &mut |v| {
                        v.parse().map(|p| self.replay = Some(p)).is_ok()
                    })
                },
                "CRABCHECK_SHRINK" => {
                    parse("0/1/true/false", &mut |v| {
                        parse_bool(v).map(|b| self.shrink.enabled = b).is_some()
                    })
                },
                "CRABCHECK_SHRINK_MAX_STEPS" => {
                    parse("an integer", &mut |v| {
                        v.parse().map(|n| self.shrink.max_steps = n).is_ok()
                    })
                },
                "CRABCHECK_SHRINK_TIMEOUT_MS" => {
                    parse("an integer", &mut |v| {
                        v.parse().map(|n| self.shrink.timeout_ms = Some(n)).is_ok()
                    })
                },
                "CRABCHECK_TARGET_TESTS" => {
                    parse("an integer", &mut |v| v.parse().map(|n| self.target.tests = n).is_ok())
                },
                "CRABCHECK_TARGET_POOL_SIZE" => {
                    parse("an integer", &mut |v| {
                        v.parse().map(|n| self.target.pool_size = n).is_ok()
                    })
                },
                "CRABCHECK_REGRESSIONS" => {
                    parse("0/1/true/false", &mut |v| {
                        parse_bool(v)
                            .map(|b| {
                                self.regressions.save = b;
                                self.regressions.replay = b;
                            })
                            .is_some()
                    })
                },
                "CRABCHECK_REGRESSIONS_DIR" => self.regressions.dir = PathBuf::from(value),
                "CRABCHECK_TRUNCATE_DEBUG_AT" => {
                    parse("an integer", &mut |v| {
                        v.parse().map(|n| self.report.truncate_debug_at = n).is_ok()
                    })
                },
                "CRABCHECK_REPORT_JSON" => self.report.json_path = Some(PathBuf::from(value)),
                other => {
                    problems.push(format!(
                        "{other} is not a crabcheck setting (see crabcheck::config); ignored"
                    ))
                },
            }
        }
        (self, problems)
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn setters_match_fields() {
        let config = Config::new()
            .tests(5)
            .seed(9)
            .max_size(3)
            .timeout(Duration::from_millis(250))
            .no_shrink()
            .no_regressions();
        assert_eq!(config.tests, 5);
        assert_eq!(config.seed, Some(9));
        assert_eq!(config.max_size, 3);
        assert_eq!(config.timeout_duration(), Some(Duration::from_millis(250)));
        assert!(!config.shrink.enabled);
        assert!(!config.regressions.save && !config.regressions.replay);
    }

    #[test]
    fn env_overrides_and_problems() {
        let (config, problems) = Config::default().apply_env_vars(vars(&[
            ("CRABCHECK_TESTS", "12"),
            ("CRABCHECK_SEED", "42"),
            ("CRABCHECK_SHRINK", "off"),
            ("CRABCHECK_REGRESSIONS", "0"),
            ("CRABCHECK_REPLAY", " 42:7 "),
            ("CRABCHECK_MAX_SIZE", "lots"),
            ("CRABCHECK_TEST", "1"),
            ("PATH", "/bin"),
        ]));
        assert_eq!(config.tests, 12);
        assert_eq!(config.seed, Some(42));
        assert!(!config.shrink.enabled);
        assert!(!config.regressions.save);
        assert_eq!(config.replay, Some(ReplayPoint { seed: 42, iteration: 7 }));
        assert_eq!(config.max_size, 100);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].contains("CRABCHECK_MAX_SIZE=lots"));
        assert!(problems[1].contains("CRABCHECK_TEST is not a crabcheck setting"));
    }

    #[cfg(feature = "json")]
    #[test]
    fn config_file_then_variables() {
        let dir = std::env::temp_dir().join(format!("crabcheck-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cfg.json");
        std::fs::write(&path, r#"{ "tests": 7, "max_size": 3 }"#).unwrap();
        let (config, problems) = Config::default().apply_env_vars(vars(&[
            ("CRABCHECK_CONFIG", path.to_str().unwrap()),
            ("CRABCHECK_TESTS", "9"),
        ]));
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(config.tests, 9, "variables beat the file");
        assert_eq!(config.max_size, 3, "the file beats the defaults");
        let (_, problems) = Config::default()
            .apply_env_vars(vars(&[("CRABCHECK_CONFIG", "/nonexistent/cfg.json")]));
        assert_eq!(problems.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(feature = "json")]
    #[test]
    fn json_round_trip_with_defaults() {
        let config = Config::from_json(r#"{ "tests": 12, "shrink": { "max_steps": 3 } }"#).unwrap();
        assert_eq!(config.tests, 12);
        assert_eq!(config.shrink.max_steps, 3);
        assert_eq!(config.max_size, Config::default().max_size);
        let again = Config::from_json(&config.to_json()).unwrap();
        assert_eq!(again, config);
    }
}
