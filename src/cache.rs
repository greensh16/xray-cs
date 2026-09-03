//! Persistent results cache (`.xray-cache`).
//!
//! ## Why this caches diagnostics, not parse trees
//!
//! The roadmap called for caching tree-sitter parse trees. That is not
//! possible: `tree_sitter::Tree` has no serialisation in the Rust binding, and
//! no stable on-disk representation to write even if we hand-rolled one. Nor
//! would an in-memory tree cache help — within one run every file is parsed
//! exactly once already, so there is nothing to reuse.
//!
//! The user-visible goal is the same either way: do no work for a file that
//! has not changed. So the cache stores each file's **diagnostics** and skips
//! both the parse *and* the rule pass on a hit, which is strictly more saved
//! work than reusing a tree would have been.
//!
//! ## Correctness
//!
//! A stale cache is worse than no cache: it reports findings that no longer
//! exist, or hides ones that do. Two layers guard against that.
//!
//! **A global fingerprint** covers everything that changes results but is not
//! part of any one file — the xray version, the resolved config, and the job
//! script. If it does not match, the whole cache is discarded rather than
//! selectively invalidated, because reasoning about partial invalidation is
//! exactly where cache bugs live.
//!
//! **Per-file `(mtime, size)`** covers the file itself. This is the same
//! heuristic every fast build tool uses. It is not perfect — a write that
//! preserves both is invisible — which is what `--no-cache` exists for.
//!
//! Unknown rule IDs, unreadable files and malformed cache JSON all
//! degrade to "no cache", never to a wrong answer.
//!
//! ## What is deliberately not cached
//!
//! Notebooks. Their diagnostics carry a `source_override` holding the cell's
//! text, which the terminal renderer needs to draw the snippet; caching them
//! would mean storing every cell's source alongside every finding. Notebooks
//! are a small minority of the files in a typical run, so they take the
//! uncached path and the cache stays simple.

use std::collections::{HashMap, HashSet};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    config::Config,
    diagnostic::{Diagnostic, Severity},
    fix::Fix,
    job::JobScript,
    rules,
};

/// Default cache location, relative to the working directory.
pub const CACHE_FILE: &str = ".xray-cache";

/// Bumped when the on-disk shape changes incompatibly. An older or newer
/// version is treated as a miss, not an error.
const CACHE_FORMAT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct CacheFile {
    format_version: u32,
    /// Everything that affects results but is not a property of one file.
    fingerprint: String,
    entries: HashMap<String, CacheEntry>,
}

#[derive(Serialize, Deserialize, Clone)]
struct CacheEntry {
    mtime_ns: u128,
    size: u64,
    diagnostics: Vec<CachedDiagnostic>,
}

/// A [`Diagnostic`] in a form that survives a round-trip to disk.
///
/// `Diagnostic::rule_id` is `&'static str`, which cannot be deserialised, so
/// it is stored as a `String` and resolved back through the rule registry on
/// load. That doubles as a validity check: a cache written by a version with a
/// rule this build does not have is rejected rather than half-understood.
#[derive(Serialize, Deserialize, Clone)]
struct CachedDiagnostic {
    rule_id: String,
    severity: Severity,
    line: usize,
    column: usize,
    message: String,
    suggestion: Option<String>,
    fix_hint: Option<String>,
    fix: Option<Fix>,
    url: Option<String>,
}

impl CachedDiagnostic {
    fn from_diagnostic(d: &Diagnostic) -> Self {
        Self {
            rule_id: d.rule_id.to_string(),
            severity: d.severity,
            line: d.line,
            column: d.column,
            message: d.message.clone(),
            suggestion: d.suggestion.clone(),
            fix_hint: d.fix_hint.clone(),
            fix: d.fix.clone(),
            url: d.url.clone(),
        }
    }

    /// `None` when the rule ID is not one this build knows.
    fn into_diagnostic(self, path: &str) -> Option<Diagnostic> {
        let rule_id = rules::static_rule_id(&self.rule_id)?;
        let mut d = Diagnostic::new(
            rule_id,
            self.severity,
            path,
            self.line,
            self.column,
            self.message,
        );
        d.suggestion = self.suggestion;
        d.fix_hint = self.fix_hint;
        d.fix = self.fix;
        d.url = self.url;
        Some(d)
    }
}

/// The results cache for one run.
pub struct Cache {
    path: PathBuf,
    fingerprint: String,
    /// Entries loaded from disk, consulted for hits.
    loaded: HashMap<String, CacheEntry>,
    /// Entries for files this run actually re-checked.
    fresh: HashMap<String, CacheEntry>,
    /// Paths that hit, so their existing entry is carried over on save.
    ///
    /// Storing just the path rather than re-inserting the diagnostics matters:
    /// on a fully-warm run of a large corpus, copying every hit's findings
    /// into `fresh` tripled the diagnostics held in memory for no gain — the
    /// bytes are already in `loaded`.
    keep: HashSet<String>,
    pub hits: usize,
    pub misses: usize,
}

impl Cache {
    /// Load the cache, or start an empty one when it is absent, unreadable,
    /// malformed, or written under a different fingerprint.
    pub fn load(dir: &Path, config: &Config, job: Option<&JobScript>) -> Self {
        let path = dir.join(CACHE_FILE);
        let fingerprint = fingerprint(config, job);

        let loaded = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<CacheFile>(&raw).ok())
            .filter(|c| c.format_version == CACHE_FORMAT_VERSION && c.fingerprint == fingerprint)
            .map(|c| c.entries)
            .unwrap_or_default();

        Self {
            path,
            fingerprint,
            loaded,
            fresh: HashMap::new(),
            keep: HashSet::new(),
            hits: 0,
            misses: 0,
        }
    }

    /// Cached diagnostics for `path`, if the file is unchanged since they were
    /// recorded.
    pub fn get(&self, path: &str) -> Option<Vec<Diagnostic>> {
        let entry = self.loaded.get(path)?;
        let (mtime_ns, size) = file_stamp(path)?;
        if entry.mtime_ns != mtime_ns || entry.size != size {
            return None;
        }
        // A single unknown rule ID invalidates the entry rather than silently
        // dropping that finding.
        entry
            .diagnostics
            .iter()
            .cloned()
            .map(|d| d.into_diagnostic(path))
            .collect()
    }

    /// Record this run's diagnostics for `path`.
    ///
    /// `diagnostics` must be the raw rule output, before any CLI filtering:
    /// `--disable` and `--min-severity` change per invocation and are applied
    /// to whatever the cache returns.
    pub fn insert(&mut self, path: &str, diagnostics: &[Diagnostic]) {
        let Some((mtime_ns, size)) = file_stamp(path) else {
            return;
        };
        self.fresh.insert(
            path.to_string(),
            CacheEntry {
                mtime_ns,
                size,
                diagnostics: diagnostics
                    .iter()
                    .map(CachedDiagnostic::from_diagnostic)
                    .collect(),
            },
        );
    }

    /// Carry `path`'s existing entry through to the next run without copying
    /// its diagnostics. Call on a hit.
    pub fn record_hit(&mut self, path: &str) {
        self.hits += 1;
        self.keep.insert(path.to_string());
    }

    pub fn record_miss(&mut self) {
        self.misses += 1;
    }

    /// Write the cache back.
    ///
    /// Entries come from this run only — files re-checked (`fresh`) plus files
    /// that hit (`keep`) — so paths that have gone away are pruned rather than
    /// accumulating forever.
    ///
    /// Serialised straight into the file. Building the document as a `String`
    /// first meant holding a second full copy of every cached finding, which
    /// on a large corpus was the largest single allocation in the process.
    ///
    /// Failures are reported and otherwise ignored: a cache that cannot be
    /// written must never fail the lint.
    pub fn save(&self) {
        let entries: HashMap<&str, &CacheEntry> = self
            .fresh
            .iter()
            .map(|(k, v)| (k.as_str(), v))
            .chain(
                self.keep
                    .iter()
                    .filter(|p| !self.fresh.contains_key(*p))
                    .filter_map(|p| self.loaded.get_key_value(p))
                    .map(|(k, v)| (k.as_str(), v)),
            )
            .collect();

        let file = CacheFileRef {
            format_version: CACHE_FORMAT_VERSION,
            fingerprint: &self.fingerprint,
            entries,
        };

        let write = || -> std::io::Result<()> {
            let handle = std::fs::File::create(&self.path)?;
            let mut out = BufWriter::new(handle);
            serde_json::to_writer(&mut out, &file)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            out.flush()
        };
        if let Err(e) = write() {
            eprintln!("xray: could not write {}: {e}", self.path.display());
        }
    }
}

/// Borrowed mirror of [`CacheFile`], for writing without cloning entries.
#[derive(Serialize)]
struct CacheFileRef<'a> {
    format_version: u32,
    fingerprint: &'a str,
    entries: HashMap<&'a str, &'a CacheEntry>,
}

/// `(mtime in nanoseconds, size in bytes)` for a path, or `None` if it cannot
/// be stat'd — which is treated as a miss.
fn file_stamp(path: &str) -> Option<(u128, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some((mtime, meta.len()))
}

/// Everything that changes results but is not a property of one source file.
///
/// Deliberately coarse: any change discards the whole cache. Config keys
/// interact with each other and with rule internals in ways that make
/// per-key invalidation a source of subtle staleness, and a full re-lint is
/// cheap compared with reporting a finding that no longer exists.
fn fingerprint(config: &Config, job: Option<&JobScript>) -> String {
    let mut parts = vec![
        format!("v={}", env!("CARGO_PKG_VERSION")),
        format!("rules={}", rules::all_meta().len()),
    ];

    // The config is not `Serialize`, so fingerprint the fields that reach the
    // rules. `Debug` is stable enough for a cache key: it changes whenever a
    // value does, which is the only property required.
    let mut disable: Vec<&str> = config.disable.iter().map(String::as_str).collect();
    disable.sort_unstable();
    parts.push(format!("disable={disable:?}"));

    let mut overrides: Vec<(&str, &str)> = config
        .severity_overrides
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    overrides.sort_unstable();
    parts.push(format!("sev={overrides:?}"));

    let mut ignores: Vec<(&str, &Vec<String>)> = config
        .per_file_ignores
        .iter()
        .map(|(k, v)| (k.as_str(), v))
        .collect();
    ignores.sort_by_key(|(k, _)| *k);
    parts.push(format!("perfile={ignores:?}"));

    parts.push(format!("min={:?}", config.min_severity));
    parts.push(format!(
        "paths={:?}/{:?}",
        config.paths.include, config.paths.exclude
    ));
    parts.push(format!("xarray={:?}", config.xarray.values_access_is_error));
    parts.push(format!("dask={}", config.dask.compute_call_threshold));
    parts.push(format!("numpy={}", config.numpy.flag_iterrows));
    parts.push(format!("io={}", config.io.flag_missing_compression));

    // The JOB rules read a second file, so its content is part of the key.
    match job {
        Some(j) => parts.push(format!(
            "job={}:{:?}:{:?}:{:?}:{:?}:{}",
            j.path,
            j.cpus.as_ref().map(|d| d.value),
            j.memory_bytes.as_ref().map(|d| d.value),
            j.gpu.as_ref().map(|d| &d.value),
            j.thread_env.as_ref().map(|d| &d.value),
            j.exclusive,
        )),
        None => parts.push("job=none".to_string()),
    }

    parts.join("|")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::Severity;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("xray-cache-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn diag(rule: &'static str, line: usize) -> Diagnostic {
        Diagnostic::new(rule, Severity::Warning, "f.py", line, 1, "msg")
            .with_suggestion("do it differently")
            .with_url("https://example.invalid/rule")
    }

    #[test]
    fn round_trips_diagnostics_for_an_unchanged_file() {
        let dir = tmpdir("roundtrip");
        let file = dir.join("f.py");
        std::fs::write(&file, "import numpy as np\n").unwrap();
        let path = file.to_string_lossy().to_string();
        let cfg = Config::default();

        let mut c = Cache::load(&dir, &cfg, None);
        assert!(c.get(&path).is_none(), "empty cache cannot hit");
        c.insert(&path, &[diag("NP003", 1), diag("XR001", 2)]);
        c.save();

        let c2 = Cache::load(&dir, &cfg, None);
        let hit = c2.get(&path).expect("unchanged file should hit");
        assert_eq!(hit.len(), 2);
        assert_eq!(hit[0].rule_id, "NP003");
        // The `&'static str` rule id is resolved back through the registry.
        assert!(std::ptr::eq(
            hit[0].rule_id,
            rules::static_rule_id("NP003").unwrap()
        ));
        // A URL that cannot be re-derived from the rule ID survives verbatim.
        assert_eq!(hit[0].url.as_deref(), Some("https://example.invalid/rule"));
        assert_eq!(hit[0].suggestion.as_deref(), Some("do it differently"));
    }

    #[test]
    fn a_changed_file_misses() {
        let dir = tmpdir("changed");
        let file = dir.join("f.py");
        std::fs::write(&file, "x = 1\n").unwrap();
        let path = file.to_string_lossy().to_string();
        let cfg = Config::default();

        let mut c = Cache::load(&dir, &cfg, None);
        c.insert(&path, &[diag("NP003", 1)]);
        c.save();

        // Different size — the cheapest of the two stamps to change reliably.
        std::fs::write(&file, "x = 1\ny = 2\n").unwrap();
        assert!(Cache::load(&dir, &cfg, None).get(&path).is_none());
    }

    #[test]
    fn a_changed_config_discards_the_whole_cache() {
        let dir = tmpdir("config");
        let file = dir.join("f.py");
        std::fs::write(&file, "x = 1\n").unwrap();
        let path = file.to_string_lossy().to_string();

        let mut c = Cache::load(&dir, &Config::default(), None);
        c.insert(&path, &[diag("NP003", 1)]);
        c.save();

        // Disabling a rule changes what the rules produce, so every entry
        // written under the old config is unusable.
        let mut cfg2 = Config::default();
        cfg2.disable.insert("XR001".to_string());
        assert!(Cache::load(&dir, &cfg2, None).get(&path).is_none());

        // A domain knob counts too, even though it names no rule.
        let mut cfg3 = Config::default();
        cfg3.dask.compute_call_threshold = 99;
        assert!(Cache::load(&dir, &cfg3, None).get(&path).is_none());
    }

    #[test]
    fn a_job_script_is_part_of_the_key() {
        let dir = tmpdir("job");
        let file = dir.join("f.py");
        std::fs::write(&file, "x = 1\n").unwrap();
        let path = file.to_string_lossy().to_string();
        let cfg = Config::default();

        let job_a = crate::job::parse_job_source("#SBATCH --cpus-per-task=4\n", "run.sh");
        let mut c = Cache::load(&dir, &cfg, Some(&job_a));
        c.insert(&path, &[diag("NP003", 1)]);
        c.save();

        assert!(Cache::load(&dir, &cfg, Some(&job_a)).get(&path).is_some());
        // Same script path, different allocation — JOB findings would differ.
        let job_b = crate::job::parse_job_source("#SBATCH --cpus-per-task=48\n", "run.sh");
        assert!(Cache::load(&dir, &cfg, Some(&job_b)).get(&path).is_none());
        // No job at all is a different key again.
        assert!(Cache::load(&dir, &cfg, None).get(&path).is_none());
    }

    #[test]
    fn garbage_and_unknown_rules_degrade_to_a_miss() {
        let dir = tmpdir("garbage");
        let file = dir.join("f.py");
        std::fs::write(&file, "x = 1\n").unwrap();
        let path = file.to_string_lossy().to_string();
        let cfg = Config::default();

        std::fs::write(dir.join(CACHE_FILE), "{ this is not json").unwrap();
        assert!(Cache::load(&dir, &cfg, None).get(&path).is_none());

        // A rule this build does not know invalidates the entry rather than
        // silently dropping the finding.
        let mut c = Cache::load(&dir, &cfg, None);
        c.insert(&path, &[]);
        c.save();
        let raw = std::fs::read_to_string(dir.join(CACHE_FILE)).unwrap();
        let doctored = raw.replace("\"diagnostics\":[]", "\"diagnostics\":[{\"rule_id\":\"ZZ999\",\"severity\":\"warning\",\"line\":1,\"column\":1,\"message\":\"m\",\"suggestion\":null,\"fix_hint\":null,\"fix\":null,\"url\":null}]");
        std::fs::write(dir.join(CACHE_FILE), doctored).unwrap();
        assert!(Cache::load(&dir, &cfg, None).get(&path).is_none());
    }

    #[test]
    fn entries_for_vanished_files_do_not_accumulate() {
        let dir = tmpdir("prune");
        let a = dir.join("a.py");
        let b = dir.join("b.py");
        std::fs::write(&a, "x = 1\n").unwrap();
        std::fs::write(&b, "y = 2\n").unwrap();
        let (pa, pb) = (
            a.to_string_lossy().to_string(),
            b.to_string_lossy().to_string(),
        );
        let cfg = Config::default();

        let mut c = Cache::load(&dir, &cfg, None);
        c.insert(&pa, &[diag("NP003", 1)]);
        c.insert(&pb, &[diag("NP003", 1)]);
        c.save();

        // A later run that only sees a.py writes back only a.py.
        let mut c2 = Cache::load(&dir, &cfg, None);
        assert!(c2.get(&pb).is_some(), "b.py is in the cache to begin with");
        c2.insert(&pa, &[diag("NP003", 1)]);
        c2.save();

        let c3 = Cache::load(&dir, &cfg, None);
        assert!(c3.get(&pa).is_some());
        assert!(c3.get(&pb).is_none(), "b.py should have been pruned");
    }
}
