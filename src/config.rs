use anyhow::{Context, Result};
use serde::Deserialize;
use std::{collections::HashMap, collections::HashSet, path::Path};

use crate::cli::MinSeverity;

/// Every section denies unknown fields: a mistyped key used to be parsed and
/// silently discarded, so `disabel = [...]` looked like it worked.
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Rule IDs to disable entirely (e.g. `["NP003", "IO001"]`).
    #[serde(default)]
    pub disable: HashSet<String>,

    /// Per-rule severity overrides.  Map rule ID → "hint" | "warning" | "error".
    /// Example: `{ "XR002" = "error", "NP003" = "hint" }`
    #[serde(default)]
    pub severity_overrides: HashMap<String, String>,

    /// Minimum severity to report.  Overridden by `--min-severity` / the
    /// `XRAY_MIN_SEVERITY` environment variable when either is supplied.
    #[serde(default)]
    pub min_severity: Option<MinSeverity>,

    /// Default file include/exclude globs (used when no paths are given on the CLI).
    #[serde(default)]
    pub paths: PathsConfig,

    #[serde(default)]
    pub xarray: XarrayConfig,

    #[serde(default)]
    pub dask: DaskConfig,

    #[serde(default)]
    pub numpy: NumpyConfig,

    #[serde(default)]
    pub io: IoConfig,
}

// ── [paths] ───────────────────────────────────────────────────────────────────

/// `[paths]` section — controls which files are linted by default.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathsConfig {
    /// Glob patterns to include.  Defaults to `["**/*.py"]`.
    #[serde(default = "default_include_globs")]
    pub include: Vec<String>,

    /// Glob patterns to exclude.  Applied after `include`.
    #[serde(default)]
    pub exclude: Vec<String>,
}

impl Default for PathsConfig {
    fn default() -> Self {
        Self {
            include: default_include_globs(),
            exclude: Vec::new(),
        }
    }
}

fn default_include_globs() -> Vec<String> {
    vec!["**/*.py".to_string()]
}

// ── domain configs ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct XarrayConfig {
    /// Treat .values access as error rather than warning.
    #[serde(default)]
    pub values_access_is_error: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DaskConfig {
    /// Max number of .compute() calls before flagging as suspicious.
    #[serde(default = "default_compute_threshold")]
    pub compute_call_threshold: usize,
}

impl Default for DaskConfig {
    fn default() -> Self {
        Self {
            compute_call_threshold: default_compute_threshold(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NumpyConfig {
    #[serde(default = "default_true")]
    pub flag_iterrows: bool,
}

impl Default for NumpyConfig {
    fn default() -> Self {
        Self {
            flag_iterrows: default_true(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IoConfig {
    /// Flag storage formats written without compression or chunking:
    /// IO001 (`np.save`) and IO002 (direct `netCDF4.Dataset` open).
    ///
    /// This key was previously parsed but read by no rule at all.
    #[serde(default = "default_true")]
    pub flag_missing_compression: bool,
}

impl Default for IoConfig {
    fn default() -> Self {
        Self {
            flag_missing_compression: default_true(),
        }
    }
}

fn default_compute_threshold() -> usize {
    3
}
fn default_true() -> bool {
    true
}

// ── loading ───────────────────────────────────────────────────────────────────

impl Config {
    pub fn from_file(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("Cannot read config: {}", path.display()))?;
        let mut cfg: Self = toml::from_str(&raw)
            .with_context(|| format!("Cannot parse config: {}", path.display()))?;
        cfg.normalise();
        Ok(cfg)
    }

    /// Upper-case every rule ID so that `disable = ["xr001"]` behaves the same
    /// as `disable = ["XR001"]`.  `validate()` has always compared
    /// case-insensitively, so without this a lowercase ID passed validation
    /// and then silently matched nothing.
    fn normalise(&mut self) {
        self.disable = self.disable.iter().map(|id| id.to_uppercase()).collect();
        self.severity_overrides = self
            .severity_overrides
            .drain()
            .map(|(id, sev)| (id.to_uppercase(), sev.to_lowercase()))
            .collect();
    }

    /// Walk up directories looking for xray.toml.
    pub fn from_dir(start: &str) -> Result<Self> {
        let mut dir = std::fs::canonicalize(start)?;
        loop {
            let candidate = dir.join("xray.toml");
            if candidate.exists() {
                return Self::from_file(&candidate);
            }
            if !dir.pop() {
                break;
            }
        }
        Ok(Self::default())
    }

    pub fn is_disabled(&self, rule_id: &str) -> bool {
        self.disable.contains(rule_id)
    }
}

// ── validation ────────────────────────────────────────────────────────────────

const VALID_SEVERITIES: &[&str] = &["hint", "warning", "error"];

impl Config {
    /// Validate the config against a list of known rule IDs.
    ///
    /// Returns a `Vec` of human-readable error strings.  An empty vec means
    /// the config is valid.  Callers should emit these as warnings and
    /// continue; they do **not** indicate a fatal error unless explicitly
    /// escalated by the caller.
    pub fn validate(&self, known_ids: &[&str]) -> Vec<String> {
        let mut errors = Vec::new();

        // ── disable list ──────────────────────────────────────────────────────
        for id in &self.disable {
            let id_upper = id.to_uppercase();
            if !known_ids.contains(&id_upper.as_str()) {
                errors.push(format!(
                    "unknown rule `{id}` in `disable` — run `xray --list-rules` for valid IDs"
                ));
            }
        }

        // ── severity_overrides ────────────────────────────────────────────────
        for (id, sev) in &self.severity_overrides {
            let id_upper = id.to_uppercase();
            if !known_ids.contains(&id_upper.as_str()) {
                errors.push(format!(
                    "unknown rule `{id}` in `severity_overrides` — run `xray --list-rules` for valid IDs"
                ));
            }
            let sev_lower = sev.to_lowercase();
            if !VALID_SEVERITIES.contains(&sev_lower.as_str()) {
                errors.push(format!(
                    "invalid severity `{sev}` for rule `{id}` in `severity_overrides` \
                     — must be one of: hint, warning, error"
                ));
            }
        }

        // ── dask thresholds ───────────────────────────────────────────────────
        if self.dask.compute_call_threshold == 0 {
            errors.push(
                "`dask.compute_call_threshold` must be ≥ 1 (0 would flag every .compute() call)"
                    .to_string(),
            );
        }

        errors
    }
}

// ── unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn known() -> Vec<&'static str> {
        vec!["XR001", "XR002", "NP003", "DK003", "IO001"]
    }

    #[test]
    fn valid_config_produces_no_errors() {
        let cfg = Config::default();
        assert!(cfg.validate(&known()).is_empty());
    }

    #[test]
    fn unknown_disable_rule_flagged() {
        let mut cfg = Config::default();
        cfg.disable.insert("FAKE99".to_string());
        let errs = cfg.validate(&known());
        assert!(
            errs.iter().any(|e| e.contains("FAKE99")),
            "expected error for unknown rule FAKE99"
        );
    }

    #[test]
    fn unknown_severity_override_rule_flagged() {
        let mut cfg = Config::default();
        cfg.severity_overrides
            .insert("FAKE99".to_string(), "error".to_string());
        let errs = cfg.validate(&known());
        assert!(errs.iter().any(|e| e.contains("FAKE99")));
    }

    #[test]
    fn invalid_severity_value_flagged() {
        let mut cfg = Config::default();
        cfg.severity_overrides
            .insert("XR001".to_string(), "critical".to_string());
        let errs = cfg.validate(&known());
        assert!(
            errs.iter().any(|e| e.contains("critical")),
            "expected error for invalid severity 'critical'"
        );
    }

    #[test]
    fn zero_compute_threshold_flagged() {
        let mut cfg = Config::default();
        cfg.dask.compute_call_threshold = 0;
        let errs = cfg.validate(&known());
        assert!(!errs.is_empty(), "zero threshold should produce an error");
    }

    #[test]
    fn valid_severity_override_no_error() {
        let mut cfg = Config::default();
        cfg.severity_overrides
            .insert("XR001".to_string(), "error".to_string());
        let errs = cfg.validate(&known());
        assert!(errs.is_empty());
    }
}
