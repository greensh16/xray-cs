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

    /// Inherit another config file, whose keys this one overrides.
    ///
    /// Resolved relative to the directory of the config that declares it, so a
    /// facility can publish a house profile that project configs extend:
    /// `extends = "../shared/nci.toml"`.
    ///
    /// Local paths only. A remote URL would make every lint run depend on the
    /// network, turn a CI failure into a fetch failure, and give whoever
    /// controls that URL the ability to change what your CI enforces — so
    /// vendor the file and point at it instead.
    #[serde(default)]
    pub extends: Option<String>,

    /// Rules to skip for paths matching a glob.
    ///
    /// `"tests/**" = ["*"]` disables every rule under `tests/`; naming
    /// specific IDs disables only those. Without this the only lever is the
    /// global `disable` list, so one noisy rule in one directory costs
    /// coverage everywhere.
    #[serde(default)]
    pub per_file_ignores: HashMap<String, Vec<String>>,

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

    #[serde(default)]
    pub job: JobConfig,
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

impl PathsConfig {
    /// True when the effective path configuration is the default.
    pub fn is_default(&self) -> bool {
        self.include == default_include_globs() && self.exclude.is_empty()
    }
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

/// `[job]` section — HPC submission-script cross-checking.
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct JobConfig {
    /// Glob for the submission script to cross-check against when `--job` is
    /// not given, so CI picks it up without the flag.
    ///
    /// The first match in sorted order is used — a glob's order is not
    /// specified, and a lint run that chose a different script per machine
    /// would be worse than choosing none.
    #[serde(default)]
    pub script: Option<String>,
}

fn default_compute_threshold() -> usize {
    3
}
fn default_true() -> bool {
    true
}

// ── loading ───────────────────────────────────────────────────────────────────

/// Merge a child TOML document over its parent while preserving the documented
/// collection semantics. Tables merge recursively, scalar/array values in the
/// child override their parent, and rule-disable collections are unioned.
///
/// Merging before deserialisation preserves field presence. Comparing a
/// deserialised child against Rust defaults cannot distinguish an omitted key
/// from an explicitly configured default value, which is why domain sections
/// were previously lost under `extends`.
fn merge_config_values(parent: &mut toml::Value, child: toml::Value, path: &mut Vec<String>) {
    match (parent, child) {
        (toml::Value::Table(parent_table), toml::Value::Table(child_table)) => {
            for (key, child_value) in child_table {
                path.push(key.clone());
                if let Some(parent_value) = parent_table.get_mut(&key) {
                    merge_config_values(parent_value, child_value, path);
                } else {
                    parent_table.insert(key, child_value);
                }
                path.pop();
            }
        }
        (toml::Value::Array(parent_items), toml::Value::Array(child_items))
            if path.as_slice() == ["disable"]
                || (path.len() == 2 && path[0] == "per_file_ignores") =>
        {
            for item in child_items {
                if !parent_items.contains(&item) {
                    parent_items.push(item);
                }
            }
        }
        (parent_value, child_value) => *parent_value = child_value,
    }
}

impl Config {
    pub fn from_file(path: &Path) -> Result<Self> {
        let merged = Self::from_file_inner(path, &mut Vec::new())?;
        let mut cfg: Self = merged
            .try_into()
            .with_context(|| format!("Cannot parse config: {}", path.display()))?;
        cfg.normalise();
        Ok(cfg)
    }

    /// `seen` carries the inheritance chain so a cycle is reported rather than
    /// recursed into until the stack runs out.
    fn from_file_inner(path: &Path, seen: &mut Vec<std::path::PathBuf>) -> Result<toml::Value> {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if seen.contains(&canonical) {
            let chain: Vec<String> = seen
                .iter()
                .map(|p| p.display().to_string())
                .chain(std::iter::once(canonical.display().to_string()))
                .collect();
            anyhow::bail!("`extends` cycle in config: {}", chain.join(" → "));
        }
        seen.push(canonical);

        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("Cannot read config: {}", path.display()))?;
        let mut child: toml::Value = toml::from_str(&raw)
            .with_context(|| format!("Cannot parse config: {}", path.display()))?;

        let parent_ref = child
            .get("extends")
            .and_then(toml::Value::as_str)
            .map(str::to_owned);
        if let Some(parent_ref) = parent_ref {
            let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
            let parent_path = base_dir.join(&parent_ref);
            let mut parent = Self::from_file_inner(&parent_path, seen).with_context(|| {
                format!(
                    "while resolving `extends = \"{parent_ref}\"` from {}",
                    path.display()
                )
            })?;
            merge_config_values(&mut parent, child, &mut Vec::new());
            child = parent;
        }

        Ok(child)
    }

    /// Rules disabled for `path` by `[per_file_ignores]`.
    ///
    /// `"*"` in the list disables every rule for matching paths.
    pub fn ignores_for_path(&self, path: &str) -> HashSet<String> {
        let p = Path::new(path);
        let mut out = HashSet::new();
        for (pattern, ids) in &self.per_file_ignores {
            let Ok(g) = glob::Pattern::new(pattern) else {
                continue;
            };
            if g.matches_path(p) || g.matches(path) {
                for id in ids {
                    out.insert(id.to_uppercase());
                }
            }
        }
        out
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
        self.per_file_ignores = self
            .per_file_ignores
            .drain()
            .map(|(glob, ids)| (glob, ids.iter().map(|i| i.to_uppercase()).collect()))
            .collect();
    }

    /// Walk up directories looking for xray.toml.
    pub fn from_dir(start: &str) -> Result<Self> {
        match Self::find_config_file(start) {
            Some(path) => Self::from_file(&path),
            None => Ok(Self::default()),
        }
    }

    /// The `xray.toml` that [`Config::from_dir`] would load, if any.
    ///
    /// Exposed so `xray doctor` can report *which* config is in effect —
    /// "walks up from the current directory" is not an answer a user can act
    /// on when the wrong file is being picked up.
    pub fn find_config_file(start: &str) -> Option<std::path::PathBuf> {
        let mut dir = std::fs::canonicalize(start).ok()?;
        loop {
            let candidate = dir.join("xray.toml");
            if candidate.exists() {
                return Some(candidate);
            }
            if !dir.pop() {
                break;
            }
        }
        None
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

    fn config_tmpdir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("xray-config-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

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

    #[test]
    fn extends_inherits_domain_settings_and_merges_nested_keys() {
        let dir = config_tmpdir("inherit-domains");
        let parent = dir.join("parent.toml");
        let child = dir.join("child.toml");
        std::fs::write(
            &parent,
            r#"
disable = ["XR001"]

[per_file_ignores]
"**/tests/**" = ["XR002"]

[paths]
include = ["parent/**/*.py"]
exclude = ["parent/generated/**"]

[xarray]
values_access_is_error = true

[dask]
compute_call_threshold = 10

[numpy]
flag_iterrows = false

[io]
flag_missing_compression = false

[job]
script = "jobs/*.sh"
"#,
        )
        .unwrap();
        std::fs::write(
            &child,
            r#"
extends = "parent.toml"
disable = ["DK001"]

[per_file_ignores]
"**/tests/**" = ["DK002"]

[paths]
exclude = ["child/generated/**"]
"#,
        )
        .unwrap();

        let cfg = Config::from_file(&child).unwrap();
        assert!(cfg.disable.contains("XR001"));
        assert!(cfg.disable.contains("DK001"));
        assert_eq!(cfg.paths.include, vec!["parent/**/*.py"]);
        assert_eq!(cfg.paths.exclude, vec!["child/generated/**"]);
        assert!(cfg.xarray.values_access_is_error);
        assert_eq!(cfg.dask.compute_call_threshold, 10);
        assert!(!cfg.numpy.flag_iterrows);
        assert!(!cfg.io.flag_missing_compression);
        assert_eq!(cfg.job.script.as_deref(), Some("jobs/*.sh"));
        let ignored = &cfg.per_file_ignores["**/tests/**"];
        assert!(ignored.contains(&"XR002".to_string()));
        assert!(ignored.contains(&"DK002".to_string()));
    }

    #[test]
    fn extends_respects_explicit_default_valued_overrides() {
        let dir = config_tmpdir("explicit-defaults");
        let parent = dir.join("parent.toml");
        let child = dir.join("child.toml");
        std::fs::write(
            &parent,
            "[paths]\ninclude = [\"parent/**/*.py\"]\n[xarray]\nvalues_access_is_error = true\n",
        )
        .unwrap();
        std::fs::write(
            &child,
            "extends = \"parent.toml\"\n[paths]\ninclude = [\"**/*.py\"]\n[xarray]\nvalues_access_is_error = false\n",
        )
        .unwrap();

        let cfg = Config::from_file(&child).unwrap();
        assert_eq!(cfg.paths.include, vec!["**/*.py"]);
        assert!(!cfg.xarray.values_access_is_error);
    }
}
