use anyhow::Result;
use ariadne::{Color, Label, Report, ReportKind, Source};
use glob::MatchOptions;
use rayon::prelude::*;
use serde_json::{Value, json};
use std::collections::HashMap;

use crate::{
    cli::{Cli, MinSeverity, OutputFormat},
    config::Config,
    diagnostic::{Diagnostic, FileResults, RunResults, Severity},
    diff, fix,
    ignore::IgnorePatterns,
    job::{self, JobScript},
    notebook, parser, rules,
};

/// Stable JSON schema version. Increment when the output object shape changes
/// in a backwards-incompatible way.
pub const JSON_SCHEMA_VERSION: &str = "1";

const GLOB_OPTS: MatchOptions = MatchOptions {
    case_sensitive: true,
    require_literal_separator: false,
    require_literal_leading_dot: false,
};

pub fn run(cli: &Cli, config: &Config) -> Result<RunResults> {
    if cli.list_rules {
        print_rule_list();
        return Ok(RunResults::default());
    }

    // ── Config validation ─────────────────────────────────────────────────────
    let all_ids: Vec<&str> = rules::all_meta().iter().map(|m| m.id).collect();
    for msg in config.validate(&all_ids) {
        eprintln!("xray: config warning: {msg}");
    }

    let paths = resolve_paths(cli, config, &cli.paths)?;

    // Parsed once for the whole run, not once per file: the submission script
    // is the same for every Python file it launches.
    let job_script = job::resolve_job_script(cli.job.as_deref(), config.job.script.as_deref())?;
    if let Some(ref j) = job_script
        && !j.has_directives
    {
        eprintln!(
            "xray: warning: {} contains no #SBATCH or #PBS directives — the JOB rules have nothing to check against",
            j.path
        );
    }

    // ── Lint files in parallel ────────────────────────────────────────────────
    // Each worker reports whether its file imported a GPU library alongside its
    // diagnostics: JOB004 is a question about the whole run, not about any one
    // file, so it cannot be answered inside the per-file pass.
    let linted: Vec<(FileResults, bool)> = paths
        .par_iter()
        .filter_map(|path| {
            if path.ends_with(".ipynb") {
                lint_notebook(path, config, cli, job_script.as_ref())
            } else {
                lint_python(path, config, cli, job_script.as_ref())
            }
        })
        .collect();

    let any_gpu_import = linted.iter().any(|(_, gpu)| *gpu);
    let mut file_results: Vec<FileResults> = linted.into_iter().map(|(f, _)| f).collect();

    // ── JOB004: one finding per run, reported against the job script ──────────
    if let Some(ref job) = job_script
        && let Some(diag) = rules::job::job004(any_gpu_import, job, config)
    {
        let mut diags = vec![diag];
        apply_filters(&mut diags, config, cli);
        if !diags.is_empty() {
            file_results.push(FileResults {
                path: job.path.clone(),
                diagnostics: diags,
            });
        }
    }

    let results = RunResults {
        files: file_results,
        paths: paths.clone(),
    };

    match cli.format {
        OutputFormat::Text => render_text(&results, &paths),
        OutputFormat::Json => render_json(&results)?,
        OutputFormat::Sarif => render_sarif(&results)?,
        OutputFormat::GitlabCodequality => render_gitlab_codequality(&results)?,
    }

    if cli.stats {
        print_stats(&results);
    }

    Ok(results)
}

/// Resolve the set of files to act on.
///
/// Priority: `--diff` > explicit paths > `[paths].include`, then
/// `[paths].exclude` globs and `.xrayignore`. Shared by `run` and `run_fix` so
/// `xray fix` never operates on a different file set than `xray` reports on.
pub fn resolve_paths(cli: &Cli, config: &Config, explicit: &[String]) -> Result<Vec<String>> {
    let mut paths = if let Some(ref git_ref) = cli.diff {
        diff::changed_python_files(git_ref)?
    } else if explicit.is_empty() {
        collect_paths(&config.paths.include)?
    } else {
        collect_paths(explicit)?
    };

    // Exclude globs are not applied to `--diff` lists, which are already a
    // precise set of changed files.
    if cli.diff.is_none() && !config.paths.exclude.is_empty() {
        let exclude_pats: Vec<glob::Pattern> = config
            .paths
            .exclude
            .iter()
            .filter_map(|p| glob::Pattern::new(p).ok())
            .collect();
        paths.retain(|p| {
            let path = std::path::Path::new(p);
            !exclude_pats
                .iter()
                .any(|pat| pat.matches_path_with(path, GLOB_OPTS))
        });
    }

    let ignore = IgnorePatterns::load(".");
    paths.retain(|p| !ignore.is_ignored(p));
    Ok(paths)
}

// ── per-file lint helpers ─────────────────────────────────────────────────────

/// Lint a single `.py` (or other Python) file.
fn lint_python(
    path: &str,
    config: &Config,
    cli: &Cli,
    job: Option<&JobScript>,
) -> Option<(FileResults, bool)> {
    match parser::parse_file(path) {
        Ok(parsed) => {
            let mut diags = rules::run_all_with_job(&parsed, path, config, job);
            apply_filters(&mut diags, config, cli);
            let imports_gpu = parsed.imports.gpu;
            Some((
                FileResults {
                    path: path.to_string(),
                    diagnostics: diags,
                },
                imports_gpu,
            ))
        }
        Err(e) => {
            eprintln!("xray: could not parse {path}: {e}");
            None
        }
    }
}

/// Lint all code cells in a `.ipynb` notebook file.
///
/// All cell diagnostics are collected into a single [`FileResults`] entry so
/// that the notebook counts as one linted "file" in the summary.  Each
/// diagnostic's `file` field encodes the cell location (e.g.
/// `notebook.ipynb:cell[3]`) and its `source_override` holds the cell source
/// text for use by the ariadne renderer.
fn lint_notebook(
    path: &str,
    config: &Config,
    cli: &Cli,
    job: Option<&JobScript>,
) -> Option<(FileResults, bool)> {
    let cells = match notebook::parse_notebook(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("xray: could not parse notebook {path}: {e}");
            return None;
        }
    };

    let mut all_diags: Vec<Diagnostic> = Vec::new();
    let mut imports_gpu = false;

    for cell in cells {
        let cell_source = cell.source.clone();
        imports_gpu |= cell.parsed.imports.gpu;
        let mut diags = rules::run_all_with_job(&cell.parsed, &cell.label, config, job);

        // Attach the cell source so `render_text` can display correct context,
        // and restore the real notebook path — a `nb.ipynb:cell[3]` label is
        // not a resolvable location for SARIF or GitLab consumers.
        for d in &mut diags {
            d.source_override = Some(cell_source.clone());
            d.file = path.to_string();
            d.cell = Some(cell.index);
        }

        apply_filters(&mut diags, config, cli);
        all_diags.extend(diags);
    }

    Some((
        FileResults {
            path: path.to_string(),
            diagnostics: all_diags,
        },
        imports_gpu,
    ))
}

/// Apply config severity overrides, CLI disable list, and min-severity filter
/// to a set of diagnostics.  Extracted to avoid duplicating the logic between
/// `lint_python` and `lint_notebook`.
fn apply_filters(diags: &mut Vec<Diagnostic>, config: &Config, cli: &Cli) {
    // Rule IDs are compared case-insensitively everywhere: `--disable xr001`
    // and `disable = ["xr001"]` now behave like the canonical upper-case form.
    let cli_disabled: Vec<String> = cli.disable.iter().map(|id| id.to_uppercase()).collect();
    let min = effective_min_severity(cli, config);

    apply_severity_overrides(diags, config);
    diags.retain(|d| !cli_disabled.iter().any(|id| id == d.rule_id));
    diags.retain(|d| severity_passes(&d.severity, &min));
}

/// Apply `[severity_overrides]` in place.
pub fn apply_severity_overrides(diags: &mut [Diagnostic], config: &Config) {
    for diag in diags.iter_mut() {
        if let Some(sev_str) = config.severity_overrides.get(diag.rule_id)
            && let Some(sev) = parse_severity(sev_str)
        {
            diag.severity = sev;
        }
    }
}

/// Config-only filtering, for callers that have no CLI flags to consult —
/// currently the LSP server, which previously published raw rule output and so
/// ignored `[severity_overrides]` and `min_severity` entirely.
pub fn apply_config_filters(diags: &mut Vec<Diagnostic>, config: &Config) {
    apply_severity_overrides(diags, config);
    if let Some(min) = config.min_severity {
        diags.retain(|d| severity_passes(&d.severity, &min));
    }
}

/// `--min-severity` (or `XRAY_MIN_SEVERITY`) wins over `min_severity` in
/// xray.toml; with neither set, every severity is reported.
pub fn effective_min_severity(cli: &Cli, config: &Config) -> MinSeverity {
    cli.min_severity
        .or(config.min_severity)
        .unwrap_or(MinSeverity::Hint)
}

// ── format: text ──────────────────────────────────────────────────────────────

fn render_text(results: &RunResults, _paths: &[String]) {
    // Diagnostics go to stdout so `xray > report.txt` captures them, matching
    // the JSON/SARIF/GitLab renderers.  Only operational messages (parse
    // failures, config warnings) stay on stderr.
    let mut cache: HashMap<&str, String> = HashMap::new();

    for diag in results.all_diagnostics() {
        // For notebook cell diagnostics `diag.file` is a display label like
        // `notebook.ipynb:cell[3]` that cannot be read from disk — use the
        // pre-populated `source_override` instead.
        // Read each file once, not once per diagnostic.
        let source_text = if let Some(ref src) = diag.source_override {
            src.clone()
        } else {
            cache
                .entry(diag.file.as_str())
                .or_insert_with(|| {
                    // Normalise CRLF exactly as `parser::parse_source` does.
                    // Diagnostic columns are byte offsets into the normalised
                    // source; rendering against the raw file drifted every
                    // ariadne label by one char per preceding line.
                    std::fs::read_to_string(&diag.file)
                        .unwrap_or_default()
                        .replace("\r\n", "\n")
                })
                .clone()
        };

        let kind = match diag.severity {
            Severity::Error => ReportKind::Error,
            Severity::Warning => ReportKind::Warning,
            Severity::Hint => ReportKind::Advice,
        };

        let offset = line_col_to_offset(&source_text, diag.line, diag.column);

        let location = diag.display_location();
        let mut report = Report::build(kind, (location.clone(), offset..offset + 1))
            .with_code(diag.rule_id)
            .with_message(&diag.message);

        report = report.with_label(
            Label::new((location.clone(), offset..offset + 1))
                .with_message(&diag.message)
                .with_color(match diag.severity {
                    Severity::Error => Color::Red,
                    Severity::Warning => Color::Yellow,
                    Severity::Hint => Color::Cyan,
                }),
        );

        if let Some(ref suggestion) = diag.suggestion {
            report = report.with_help(suggestion.clone());
        }
        if let Some(ref fix) = diag.fix_hint {
            let note = match diag.url {
                Some(url) => format!("fix: {fix}  |  docs: {url}"),
                None => format!("fix: {fix}"),
            };
            report = report.with_note(note);
        } else if let Some(url) = diag.url {
            report = report.with_note(format!("docs: {url}"));
        }

        report
            .finish()
            .print((location, Source::from(&source_text)))
            .ok();
    }

    let total = results.total();
    if total == 0 {
        println!("xray: no issues found.");
    } else {
        println!(
            "\nxray: {} issue{} found.",
            total,
            if total == 1 { "" } else { "s" }
        );
    }
}

// ── format: json ──────────────────────────────────────────────────────────────

fn render_json(results: &RunResults) -> Result<()> {
    println!("{}", build_json(results)?);
    Ok(())
}

/// Build the stable JSON output envelope (exposed for testing).
///
/// Schema:
/// ```json
/// {
///   "schema_version": "1",
///   "diagnostics": [...],
///   "summary": { "total": N, "errors": N, "warnings": N, "hints": N }
/// }
/// ```
pub fn build_json(results: &RunResults) -> Result<String> {
    let diags: Vec<_> = results.all_diagnostics().collect();
    let errors = diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    let warnings = diags
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .count();
    let hints = diags
        .iter()
        .filter(|d| d.severity == Severity::Hint)
        .count();
    let envelope = json!({
        "schema_version": JSON_SCHEMA_VERSION,
        "diagnostics": diags,
        "summary": {
            "total": diags.len(),
            "errors": errors,
            "warnings": warnings,
            "hints": hints,
        }
    });
    Ok(serde_json::to_string_pretty(&envelope)?)
}

// ── format: sarif ─────────────────────────────────────────────────────────────

/// Render SARIF 2.1.0.
/// Printed to stdout so it can be piped or redirected to a file for upload
/// to GitHub Code Scanning / other SARIF consumers.
pub fn render_sarif(results: &RunResults) -> Result<()> {
    println!("{}", build_sarif_json(results)?);
    Ok(())
}

/// Build the SARIF JSON value (exposed for testing).
pub fn build_sarif_json(results: &RunResults) -> Result<String> {
    let meta = rules::all_meta();

    // ── tool.driver.rules ─────────────────────────────────────────────────────
    let rules_arr: Vec<Value> = meta
        .iter()
        .map(|m| {
            let level = severity_to_sarif_level(m.severity);
            let rule = json!({
                "id": m.id,
                "name": m.name,
                "shortDescription": { "text": m.description },
                "defaultConfiguration": { "level": level },
            });
            // helpUri only when the rule has a URL — we don't have one statically
            // here, so we leave it out for now (added per-result instead)
            rule
        })
        .collect();

    // ── results ───────────────────────────────────────────────────────────────
    let results_arr: Vec<Value> = results.all_diagnostics().map(build_sarif_result).collect();

    let sarif = json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "xray",
                    "version": env!("CARGO_PKG_VERSION"),
                    "informationUri": "https://github.com/greensh16/xray-cs",
                    "rules": rules_arr,
                }
            },
            "results": results_arr,
            "originalUriBaseIds": {
                "SRCROOT": { "description": { "text": "Directory xray was run from" } }
            },
        }]
    });

    Ok(serde_json::to_string_pretty(&sarif)?)
}

fn build_sarif_result(d: &Diagnostic) -> Value {
    let level = severity_to_sarif_level(d.severity);
    let mut result = json!({
        "ruleId": d.rule_id,
        "level": level,
        "message": { "text": d.message },
        "locations": [{
            "physicalLocation": {
                "artifactLocation": {
                    "uri": d.file,
                    "uriBaseId": "SRCROOT",
                },
                "region": {
                    "startLine": d.line,
                    "startColumn": d.column,
                }
            }
        }],
    });

    // A mechanical fix becomes a real SARIF `artifactChange`, which
    // SARIF-aware tooling can apply. Previously every fix carried an empty
    // `artifactChanges` array, which is well-formed but actionable by nothing.
    if let Some(ref fix) = d.fix {
        result["fixes"] = json!([{
            "description": { "text": fix.description },
            "artifactChanges": [{
                "artifactLocation": { "uri": d.file, "uriBaseId": "SRCROOT" },
                "replacements": [{
                    "deletedRegion": {
                        "startLine": fix.start_line,
                        "startColumn": fix.start_column,
                        "endLine": fix.end_line,
                        "endColumn": fix.end_column,
                    },
                    "insertedContent": { "text": fix.replacement },
                }],
            }],
        }]);
    } else if let Some(ref hint) = d.fix_hint {
        // Advisory only: no verified rewrite, so no artifactChanges to offer.
        result["fixes"] = json!([{ "description": { "text": hint } }]);
    }

    // Attach docs URL as a related location / help URI
    if let Some(url) = d.url {
        result["helpUri"] = json!(url);
    }

    // Notebook cell index, for consumers that can use it.
    if let Some(cell) = d.cell {
        result["properties"] = json!({ "notebookCell": cell });
    }

    result
}

fn severity_to_sarif_level(sev: Severity) -> &'static str {
    match sev {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Hint => "note",
    }
}

// ── format: gitlab-codequality ────────────────────────────────────────────────

/// Render GitLab Code Quality JSON.
/// Upload as a CI artifact with `codequality` report type.
pub fn render_gitlab_codequality(results: &RunResults) -> Result<()> {
    println!("{}", build_gitlab_json(results)?);
    Ok(())
}

/// Build the GitLab Code Quality JSON array (exposed for testing).
pub fn build_gitlab_json(results: &RunResults) -> Result<String> {
    let entries: Vec<Value> = results.all_diagnostics().map(build_gitlab_entry).collect();
    Ok(serde_json::to_string_pretty(&entries)?)
}

fn build_gitlab_entry(d: &Diagnostic) -> Value {
    let severity = severity_to_gitlab(d.severity);
    // Fingerprint: deterministic hash of rule_id + file + position + message.
    // GitLab requires fingerprints to be unique within a report, and two
    // diagnostics from the same rule can legitimately share a line (different
    // columns), so the column and message are part of the hash.
    let fingerprint = format!(
        "{:x}",
        simple_hash(&format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
            d.rule_id, d.file, d.line, d.column, d.message
        ))
    );

    json!({
        "description": d.message,
        "check_name": format!("xray/{}", d.rule_id),
        "fingerprint": fingerprint,
        "severity": severity,
        "location": {
            "path": d.file,
            "lines": { "begin": d.line }
        }
    })
}

fn severity_to_gitlab(sev: Severity) -> &'static str {
    match sev {
        Severity::Error => "critical",
        Severity::Warning => "major",
        Severity::Hint => "info",
    }
}

/// A deterministic, dependency-free hash for fingerprinting diagnostics.
/// FNV-1 64-bit (multiply, then xor), which is sufficient for a stable CI
/// fingerprint — this is not a cryptographic hash.
fn simple_hash(s: &str) -> u64 {
    const FNV_OFFSET: u64 = 14_695_981_039_346_656_037;
    const FNV_PRIME: u64 = 1_099_511_628_211;
    s.bytes().fold(FNV_OFFSET, |acc, b| {
        acc.wrapping_mul(FNV_PRIME) ^ (b as u64)
    })
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn parse_severity(s: &str) -> Option<Severity> {
    match s.to_lowercase().as_str() {
        "hint" => Some(Severity::Hint),
        "warning" => Some(Severity::Warning),
        "error" => Some(Severity::Error),
        _ => None,
    }
}

fn severity_passes(sev: &Severity, min: &MinSeverity) -> bool {
    match min {
        MinSeverity::Hint => true,
        MinSeverity::Warning => *sev >= Severity::Warning,
        MinSeverity::Error => *sev >= Severity::Error,
    }
}

/// Emit every rule's metadata as JSON.
///
/// This is the source of truth for generated documentation: the README table,
/// `docs/rules/*.md` and the wiki all describe the same 33 rules, and keeping
/// four hand-maintained copies in sync is what produced the drift found in the
/// v1.0 review. Fields are joined from `RuleMeta` (id, name, severity,
/// description) and `ExplainEntry` (domain, docs URL, auto-fix eligibility).
pub fn print_rule_list_json() -> Result<()> {
    let entries: Vec<Value> = rules::all_meta()
        .iter()
        .map(|m| {
            let ex = crate::explain::entry_for(m.id);
            json!({
                "id": m.id,
                "name": m.name,
                "severity": m.severity.to_string(),
                "description": m.description,
                "domain": ex.map(|e| e.domain),
                "url": ex.and_then(|e| e.url),
                "fix_eligible": crate::fix::is_fixable(m.id),
            })
        })
        .collect();

    let out = json!({
        "schema_version": JSON_SCHEMA_VERSION,
        "tool": { "name": "xray", "version": env!("CARGO_PKG_VERSION") },
        "rules": entries,
    });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

pub fn print_rule_list() {
    let meta = rules::all_meta();
    println!("{:<8} {:<10} {:<35} DESCRIPTION", "ID", "SEVERITY", "NAME");
    println!("{}", "─".repeat(100));
    for m in meta {
        println!(
            "{:<8} {:<10} {:<35} {}",
            m.id,
            format!("{}", m.severity),
            m.name,
            m.description
        );
    }
}

/// Print per-rule and per-file summary tables (activated by --stats).
fn print_stats(results: &RunResults) {
    let total = results.total();
    let file_count = results.files.len();

    eprintln!();
    eprintln!(
        "  xray stats ─── {} file{}, {} issue{}",
        file_count,
        if file_count == 1 { "" } else { "s" },
        total,
        if total == 1 { "" } else { "s" }
    );

    if total == 0 {
        return;
    }

    let mut rule_counts: HashMap<&'static str, usize> = HashMap::new();
    for diag in results.all_diagnostics() {
        *rule_counts.entry(diag.rule_id).or_insert(0) += 1;
    }
    let mut rule_vec: Vec<_> = rule_counts.iter().collect();
    rule_vec.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));

    let meta = rules::all_meta();
    let meta_map: HashMap<_, _> = meta.iter().map(|m| (m.id, m)).collect();

    eprintln!();
    eprintln!("  {:<8}  {:>5}  NAME", "RULE", "COUNT");
    eprintln!("  {}  {}  {}", "─".repeat(8), "─".repeat(5), "─".repeat(35));
    for (id, count) in &rule_vec {
        let name = meta_map.get(*id).map(|m| m.name).unwrap_or("unknown");
        eprintln!("  {:<8}  {:>5}  {}", id, count, name);
    }

    let files_with_issues: Vec<_> = results
        .files
        .iter()
        .filter(|fr| !fr.diagnostics.is_empty())
        .collect();

    if !files_with_issues.is_empty() {
        eprintln!();
        eprintln!("  {:<52}  {:>6}", "FILE", "ISSUES");
        eprintln!("  {}  {}", "─".repeat(52), "─".repeat(6));
        for fr in &files_with_issues {
            let trimmed = fr.path.trim_start_matches("./");
            let display = if trimmed.len() > 52 {
                format!("…{}", &trimmed[trimmed.len() - 51..])
            } else {
                trimmed.to_string()
            };
            eprintln!("  {:<52}  {:>6}", display, fr.diagnostics.len());
        }
    }

    eprintln!();
}

/// Public re-export of `collect_paths` for integration testing of glob edge cases.
pub fn collect_paths_pub(patterns: &[String]) -> Result<Vec<String>> {
    collect_paths(patterns)
}

/// Source file extensions xray knows how to lint.
pub const LINTABLE_EXTENSIONS: &[&str] = &["py", "ipynb"];

pub fn is_lintable_path(path: &str) -> bool {
    is_lintable(path)
}

fn is_lintable(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| LINTABLE_EXTENSIONS.contains(&e))
}

/// Expand one CLI argument or config glob into concrete file paths.
///
/// Three argument shapes are accepted:
/// * a file — used as-is
/// * a directory — expanded to every lintable file beneath it
/// * a glob pattern — expanded by `glob`; directories it yields are themselves
///   expanded, so `src/*` behaves sensibly
fn expand_pattern(pattern: &str, out: &mut Vec<String>) -> Result<()> {
    let as_path = std::path::Path::new(pattern);

    if as_path.is_file() {
        out.push(normalise(pattern));
        return Ok(());
    }

    if as_path.is_dir() {
        expand_dir(pattern, out)?;
        return Ok(());
    }

    for entry in glob::glob(pattern)
        .map_err(|e| anyhow::anyhow!("invalid glob pattern `{pattern}`: {e}"))?
        .flatten()
    {
        let Some(s) = entry.to_str() else { continue };
        if entry.is_dir() {
            expand_dir(s, out)?;
        } else {
            out.push(normalise(s));
        }
    }
    Ok(())
}

/// Recursively collect every lintable file under `dir`.
fn expand_dir(dir: &str, out: &mut Vec<String>) -> Result<()> {
    let trimmed = dir.trim_end_matches('/');
    for ext in LINTABLE_EXTENSIONS {
        let pattern = format!("{trimmed}/**/*.{ext}");
        for entry in glob::glob(&pattern)
            .map_err(|e| anyhow::anyhow!("invalid glob pattern `{pattern}`: {e}"))?
            .flatten()
        {
            if entry.is_file()
                && let Some(s) = entry.to_str()
            {
                out.push(normalise(s));
            }
        }
    }
    Ok(())
}

/// Strip a leading `./` so the same file discovered via different patterns
/// deduplicates, and so SARIF consumers get clean relative URIs.
fn normalise(path: &str) -> String {
    path.trim_start_matches("./").to_string()
}

fn collect_paths(patterns: &[String]) -> Result<Vec<String>> {
    let mut paths = Vec::new();
    for pattern in patterns {
        expand_pattern(pattern, &mut paths)?;
    }
    dedupe(&mut paths);
    Ok(paths)
}

/// Remove duplicate paths while preserving discovery order.  Overlapping
/// arguments (`xray src/ src/a.py`) or overlapping `[paths].include` globs
/// would otherwise lint the same file — and report every diagnostic — twice.
fn dedupe(paths: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    paths.retain(|p| seen.insert(p.clone()));
}

fn line_col_to_offset(source: &str, line: usize, col: usize) -> usize {
    let mut char_offset = 0usize;
    for (i, l) in source.lines().enumerate() {
        if i + 1 == line {
            // col is a byte-based column from tree-sitter; convert to char count
            let byte_col = col.saturating_sub(1).min(l.len());
            let char_col = l[..byte_col].chars().count();
            return char_offset + char_col;
        }
        // ariadne uses char-based offsets, so count chars (not bytes) per line
        char_offset += l.chars().count() + 1;
    }
    char_offset
}

// ── xray fix ──────────────────────────────────────────────────────────────────

/// Summary of a `xray fix` run.
#[derive(Debug, Default)]
pub struct FixSummary {
    pub files_changed: usize,
    pub fixes_applied: usize,
    pub fixes_skipped: usize,
    /// Notebooks encountered, which are never rewritten.
    pub notebooks_skipped: usize,
}

/// Apply every available auto-fix across the resolved file set.
///
/// Prints a diff for each file it touches. With `dry_run` nothing is written —
/// the diff is the whole output.
///
/// Notebooks are reported and skipped: fix offsets are into a cell's extracted
/// source, and splicing those back into the `.ipynb` JSON is a different
/// problem from editing a `.py` file.
pub fn run_fix(
    cli: &Cli,
    config: &Config,
    explicit: &[String],
    dry_run: bool,
) -> Result<FixSummary> {
    let all_ids: Vec<&str> = rules::all_meta().iter().map(|m| m.id).collect();
    for msg in config.validate(&all_ids) {
        eprintln!("xray: config warning: {msg}");
    }

    let paths = resolve_paths(cli, config, explicit)?;
    let mut summary = FixSummary::default();

    for path in &paths {
        if path.ends_with(".ipynb") {
            summary.notebooks_skipped += 1;
            continue;
        }

        // Read raw so the original line endings can be restored on write.
        let Ok(raw) = std::fs::read(path) else {
            eprintln!("xray: cannot read {path}");
            continue;
        };
        let raw = String::from_utf8_lossy(&raw).into_owned();

        let Ok(parsed) = parser::parse_source(raw.clone()) else {
            eprintln!("xray: could not parse {path} — skipping");
            continue;
        };
        let mut diags = rules::run_all(&parsed, path, config);
        // Same filters as linting, so `xray fix` changes exactly what `xray`
        // reports — a rule you disabled is not quietly rewritten anyway.
        apply_filters(&mut diags, config, cli);
        if diags.iter().all(|d| d.fix.is_none()) {
            continue;
        }

        // `parsed.source` is the CRLF-normalised text the fix offsets index.
        let outcome = fix::apply(&parsed.source, &diags);
        summary.fixes_applied += outcome.applied;
        summary.fixes_skipped += outcome.skipped_overlapping;
        if !outcome.changed() {
            continue;
        }

        summary.files_changed += 1;
        print!("{}", fix::render_diff(path, &outcome));

        if !dry_run {
            let to_write = fix::restore_line_endings(&raw, &outcome.fixed);
            if let Err(e) = std::fs::write(path, to_write) {
                eprintln!("xray: cannot write {path}: {e}");
            }
        }
    }

    let verb = if dry_run { "would apply" } else { "applied" };
    println!(
        "\nxray: {verb} {} fix(es) across {} file(s).",
        summary.fixes_applied, summary.files_changed
    );
    if summary.fixes_skipped > 0 {
        println!(
            "xray: skipped {} overlapping fix(es) — rerun to apply them.",
            summary.fixes_skipped
        );
    }
    if summary.notebooks_skipped > 0 {
        println!(
            "xray: skipped {} notebook(s) — `xray fix` does not rewrite .ipynb files.",
            summary.notebooks_skipped
        );
    }
    if dry_run && summary.files_changed > 0 {
        println!("xray: --dry-run, nothing written.");
    }
    Ok(summary)
}
