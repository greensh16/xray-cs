pub mod dask;
pub mod io;
pub mod numpy;
pub mod xarray;

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

    // Apply inline suppressions
    out.retain(|d| !file.suppressions.is_suppressed(d.rule_id, d.line));

    drop_redundant(&mut out);

    // Sort by line number for readable output
    out.sort_by_key(|d| d.line);
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

/// All rule metadata for --list-rules
pub fn all_meta() -> Vec<RuleMeta> {
    let mut meta = Vec::new();
    meta.extend(xarray::XarrayRules::meta());
    meta.extend(dask::DaskRules::meta());
    meta.extend(numpy::NumpyRules::meta());
    meta.extend(io::IoRules::meta());
    meta
}
