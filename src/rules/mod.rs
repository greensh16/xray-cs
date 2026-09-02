pub mod dask;
pub mod io;
pub mod numpy;
pub mod xarray;

use std::collections::HashSet;

use crate::{
    config::Config,
    diagnostic::{Diagnostic, RuleMeta},
    parser::ParsedFile,
};

/// Every rule set implements this trait.
pub trait RuleSet {
    fn meta() -> Vec<RuleMeta>
    where
        Self: Sized;

    fn check(file: &ParsedFile, path: &str, config: &Config) -> Vec<Diagnostic>
    where
        Self: Sized;
}

/// Run all rule sets against a single parsed file.
pub fn run_all(file: &ParsedFile, path: &str, config: &Config) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    // `[per_file_ignores]` is resolved once per file rather than consulted per
    // diagnostic: the globs are fixed for the whole run.
    let path_ignores = config.ignores_for_path(path);
    let ignore_all = path_ignores.contains("*");

    if file.imports.xarray {
        out.extend(xarray::XarrayRules::check(file, path, config));
    }
    if file.imports.dask {
        out.extend(dask::DaskRules::check(file, path, config));
    }
    if file.imports.numpy || file.imports.pandas {
        out.extend(numpy::NumpyRules::check(file, path, config));
    }
    // IO rules fire whenever any relevant library is imported.  `xarray` is in
    // the list because IO006 inspects `xr.open_dataset(engine=...)`; without it
    // that rule could never fire in a file that imports only xarray.
    if file.imports.netcdf4
        || file.imports.zarr
        || file.imports.numpy
        || file.imports.h5py
        || file.imports.xarray
    {
        out.extend(io::IoRules::check(file, path, config));
    }

    // `[per_file_ignores]` — glob-scoped disables.
    if ignore_all {
        out.clear();
    } else if !path_ignores.is_empty() {
        out.retain(|d| !path_ignores.contains(d.rule_id));
    }

    // Apply inline suppressions, recording which ones actually matched so
    // XR000 can report the ones that did not.
    let mut used: HashSet<(usize, &'static str)> = HashSet::new();
    out.retain(|d| {
        if file.suppressions.is_suppressed(d.rule_id, d.line) {
            used.insert((d.line, d.rule_id));
            false
        } else {
            true
        }
    });

    drop_redundant(&mut out);

    if !ignore_all && !config.is_disabled("XR000") && !path_ignores.contains("XR000") {
        out.extend(stale_suppressions(file, path, &used));
    }

    // Sort by line number for readable output
    out.sort_by_key(|d| d.line);
    out
}

/// XR000 — a `# xray: disable=` comment that suppressed nothing.
///
/// Suppressions rot silently: the code under them changes, the rule stops
/// firing, and the comment stays forever, hiding whatever the line grows into
/// later. Nothing else reports them, so they accumulate.
///
/// Only line-level suppressions are checked. `disable-file=` legitimately
/// covers a file that currently has no violations, and reporting it would
/// punish exactly the defensive use it exists for.
fn stale_suppressions(
    file: &ParsedFile,
    path: &str,
    used: &HashSet<(usize, &'static str)>,
) -> Vec<Diagnostic> {
    let known: HashSet<&str> = all_meta().iter().map(|m| m.id).collect();
    let mut out = Vec::new();

    let mut lines: Vec<(&usize, &HashSet<String>)> = file.suppressions.line_level.iter().collect();
    lines.sort_by_key(|(l, _)| **l);

    for (line, ids) in lines {
        let mut stale: Vec<&str> = ids
            .iter()
            .map(String::as_str)
            // An ID xray does not know is already reported by `validate()`;
            // saying it "suppressed nothing" too would be noise.
            .filter(|id| known.contains(id))
            .filter(|id| !used.iter().any(|(l, rid)| l == line && rid == id))
            .collect();
        if stale.is_empty() {
            continue;
        }
        stale.sort_unstable();
        out.push(
            Diagnostic::new(
                "XR000",
                crate::diagnostic::Severity::Hint,
                path,
                *line,
                1,
                format!(
                    "suppression for {} matched no diagnostic on this line",
                    stale.join(", ")
                ),
            )
            .with_suggestion(
                "Remove the `# xray: disable=` comment — the rule it silences no longer fires here",
            )
            .with_url("https://github.com/greensh16/xray/wiki/Suppressions"),
        );
    }
    out
}

/// Rules that report the same problem at the same place, and which of the pair
/// survives.  `.compute()` is dask's API, so DK001 owns "compute in a loop";
/// `dask.compute(...)` is more specific still, so DK002 owns that call.
///
/// Without this, one `.compute()` inside a loop in a file importing both xarray
/// and dask produced three separate errors for a single call.
const REDUNDANT_WITH: &[(&str, &str)] = &[
    // (rule to drop, rule that supersedes it when both hit the same position)
    ("XR005", "DK001"),
    ("XR005", "DK002"),
    ("DK001", "DK002"),
];

fn drop_redundant(diags: &mut Vec<Diagnostic>) {
    if diags.len() < 2 {
        return;
    }
    let positions: Vec<(&str, usize, usize)> = diags
        .iter()
        .map(|d| (d.rule_id, d.line, d.column))
        .collect();

    diags.retain(|d| {
        !REDUNDANT_WITH.iter().any(|(drop_id, keep_id)| {
            *drop_id == d.rule_id
                && positions
                    .iter()
                    .any(|(id, line, col)| id == keep_id && *line == d.line && *col == d.column)
        })
    });
}

/// Cross-domain rules that belong to no single library.
const CROSS_DOMAIN_META: &[RuleMeta] = &[RuleMeta {
    id: "XR000",
    name: "stale-suppression",
    severity: crate::diagnostic::Severity::Hint,
    description: "A `# xray: disable=` comment that suppressed nothing — the rule no longer fires here",
}];

/// All rule metadata for --list-rules
pub fn all_meta() -> Vec<RuleMeta> {
    let mut meta = Vec::new();
    meta.extend(CROSS_DOMAIN_META.iter().cloned());
    meta.extend(xarray::XarrayRules::meta());
    meta.extend(dask::DaskRules::meta());
    meta.extend(numpy::NumpyRules::meta());
    meta.extend(io::IoRules::meta());
    meta
}
