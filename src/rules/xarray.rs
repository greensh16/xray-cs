use std::sync::LazyLock;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Query, QueryCursor};

use crate::{
    bindings::Origin,
    config::Config,
    diagnostic::{Diagnostic, RuleMeta, Severity},
    parser,
    parser::{
        ParsedFile, call_is_from, has_keyword_arg, is_inside_loop, keyword_arg_present_or_unknown,
        keyword_arg_value, node_text, position,
    },
};

use super::RuleSet;

pub struct XarrayRules;

const QUERY_SRC: &str = include_str!("../../queries/xarray.scm");

/// Attribute names XR003 treats as a Dataset/DataArray dimension.
///
/// The rule used to fire for *any* attribute iterated in a `for` loop, so
/// ordinary code like `for f in self.files:` was reported as a dimension loop.
/// Restricting to a dimension vocabulary keeps the true positives (`for t in
/// ds.time:`) and drops the noise.
const DIMENSION_NAMES: &[&str] = &[
    "time",
    "lat",
    "latitude",
    "lon",
    "longitude",
    "level",
    "lev",
    "depth",
    "height",
    "plev",
    "x",
    "y",
    "z",
    "member",
    "ensemble",
    "realization",
    "band",
    "channel",
    "dims",
    "coords",
    "variables",
    "data_vars",
    "indexes",
];

/// Compiled once per process and shared across all rayon workers.
/// `Query` is `Send + Sync`; only the `QueryCursor` needs to be per-call.
/// A compilation failure is a bug in xray itself, so we fail loudly.
static QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&tree_sitter_python::LANGUAGE.into(), QUERY_SRC)
        .unwrap_or_else(|e| panic!("xray: BUG — failed to compile xarray query: {e}"))
});

impl RuleSet for XarrayRules {
    fn meta() -> Vec<RuleMeta> {
        vec![
            RuleMeta {
                id: "XR001",
                name: "open-dataset-without-chunks",
                severity: Severity::Warning,
                description: "open_dataset/open_mfdataset called without chunks= — data loads eagerly into memory",
            },
            RuleMeta {
                id: "XR002",
                name: "values-access-on-dataarray",
                severity: Severity::Warning,
                description: ".values accessed on a DataArray — materialises the full array and drops coordinates",
            },
            RuleMeta {
                id: "XR003",
                name: "loop-over-dimension",
                severity: Severity::Hint,
                description: "for-loop iterating over a Dataset/DataArray attribute — prefer vectorised operations",
            },
            RuleMeta {
                id: "XR004",
                name: "sel-with-float",
                severity: Severity::Warning,
                description: ".sel() called with a float literal — use method='nearest' or tolerance= to avoid silent misses",
            },
            RuleMeta {
                id: "XR005",
                name: "compute-in-loop",
                severity: Severity::Error,
                description: ".compute() called inside a for loop — triggers the full dask graph on every iteration",
            },
            RuleMeta {
                id: "XR006",
                name: "to-array-without-dim",
                severity: Severity::Warning,
                description: ".to_array()/.to_dataarray() called without dim= — creates an unnamed 'variable' concat dimension",
            },
            RuleMeta {
                id: "XR007",
                name: "concat-in-loop",
                severity: Severity::Error,
                description: "xr.concat called inside a for loop — O(n²) intermediate copies; collect then concat once",
            },
            RuleMeta {
                id: "XR008",
                name: "open-mfdataset-without-parallel",
                severity: Severity::Warning,
                description: "open_mfdataset called without parallel=True — files are opened serially",
            },
            RuleMeta {
                id: "XR009",
                name: "apply-ufunc-dask-allowed",
                severity: Severity::Warning,
                description: "apply_ufunc with dask='allowed' silently falls back to serial execution; use dask='parallelized'",
            },
            RuleMeta {
                id: "XR010",
                name: "merge-in-loop",
                severity: Severity::Warning,
                description: "xr.merge called inside a for loop — O(n²) cost; collect datasets then merge once",
            },
            RuleMeta {
                id: "XR011",
                name: "to-netcdf-without-encoding",
                severity: Severity::Hint,
                description: "to_netcdf() called without encoding= — data written as float64 with no compression",
            },
        ]
    }

    fn check(file: &ParsedFile, path: &str, config: &Config) -> Vec<Diagnostic> {
        let mut diags = Vec::new();
        let source = file.source.as_bytes();
        let query = &*QUERY;

        let mut cursor = QueryCursor::new();
        let root = file.tree.root_node();

        // `.sel(lat=-33.5, lon=150.2)` matches the XR004 pattern once per float
        // argument; the diagnostic is about the call, so report it once.
        let mut reported_sel_calls: std::collections::HashSet<usize> =
            std::collections::HashSet::new();

        let mut matches = cursor.matches(query, root, source);
        while let Some(m) = matches.next() {
            let pattern = m.pattern_index;
            // Patterns are 0-indexed in the order they appear in the .scm file
            match pattern {
                // XR001 — open_dataset without chunks
                0 if !config.is_disabled("XR001") => {
                    if let Some(call_node) = query
                        .capture_index_for_name("xr_open_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        if keyword_arg_present_or_unknown(call_node, source, "chunks") {
                            continue;
                        }
                        let (line, col) = position(&call_node);
                        let fn_text = query
                            .capture_index_for_name("fn_bare")
                            .and_then(|i| m.nodes_for_capture_index(i).next())
                            .or_else(|| {
                                query
                                    .capture_index_for_name("fn_attr")
                                    .and_then(|i| m.nodes_for_capture_index(i).next())
                            })
                            .map(|n| node_text(&n, source))
                            .unwrap_or("open_dataset");

                        diags.push(
                            Diagnostic::new(
                                "XR001",
                                Severity::Warning,
                                path,
                                line,
                                col,
                                format!("`{fn_text}()` called without `chunks=` — data will load eagerly into RAM"),
                            )
                            .with_suggestion("Add `chunks='auto'` or a dict matching your storage chunk layout")
                            .with_fix_hint(format!("{fn_text}(path, chunks=\"auto\")"))
                            .with_url("https://docs.xarray.dev/en/stable/user-guide/dask.html"),
                        );
                    }
                }

                // XR002 — .values access
                // Guard: skip `dict.values()` style method calls — those are the function
                // of a call node, not a bare property access on a DataArray.
                1 if !config.is_disabled("XR002") => {
                    if let Some(node) = query
                        .capture_index_for_name("xr_values_access")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        // Determine if this attribute is being *called* (e.g. d.values())
                        // by checking whether the attribute node is the `function` child
                        // of a `call` parent — if so it's a method, not a property.
                        let is_method_call = node
                            .parent()
                            .filter(|p| p.kind() == "call")
                            .and_then(|p| p.child_by_field_name("function"))
                            .map(|f| f.id() == node.id())
                            .unwrap_or(false);

                        // Receiver check: `.values` is a genuine problem on an
                        // xarray object, but `df.values` is the documented pandas
                        // idiom and a dict's `.values` is unrelated. Stay silent
                        // only when the receiver is *provably* something else —
                        // an unknown receiver (a function parameter, say) keeps
                        // the original behaviour rather than going quiet.
                        let receiver_is_other = query
                            .capture_index_for_name("xr_values_obj")
                            .and_then(|i| m.nodes_for_capture_index(i).next())
                            .and_then(|obj| file.bindings.origin_of(obj, source, &file.imports))
                            .is_some_and(|o| {
                                matches!(o, Origin::Pandas | Origin::Numpy | Origin::Plain)
                            });

                        if !is_method_call && !receiver_is_other {
                            let (line, col) = position(&node);
                            let severity = if config.xarray.values_access_is_error {
                                Severity::Error
                            } else {
                                Severity::Warning
                            };
                            diags.push(
                                Diagnostic::new(
                                    "XR002",
                                    severity,
                                    path,
                                    line,
                                    col,
                                    "`.values` materialises the full array and discards all coordinate metadata",
                                )
                                .with_suggestion("Use `.to_numpy()` (explicit) or `.data` (keeps dask arrays lazy)")
                                .with_url("https://docs.xarray.dev/en/stable/generated/xarray.DataArray.to_numpy.html"),
                            );
                        }
                    }
                }

                // XR003 — loop over dimension
                2 if !config.is_disabled("XR003") => {
                    if let Some(iter_node) = query
                        .capture_index_for_name("xr_loop_iter")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        let dim = query
                            .capture_index_for_name("xr_loop_dim")
                            .and_then(|i| m.nodes_for_capture_index(i).next())
                            .map(|n| node_text(&n, source))
                            .unwrap_or("dimension");
                        if !DIMENSION_NAMES.contains(&dim) {
                            continue;
                        }
                        let (line, col) = position(&iter_node);
                        diags.push(
                            Diagnostic::new(
                                "XR003",
                                Severity::Hint,
                                path,
                                line,
                                col,
                                format!("Iterating over `.{dim}` in a Python loop — consider `.map()`, `.apply_ufunc()`, or vectorised indexing"),
                            )
                            .with_suggestion(format!(
                                "Use `ds.isel({dim}=slice(...))` or `xr.apply_ufunc` for vectorised operations"
                            ))
                            .with_url("https://docs.xarray.dev/en/stable/user-guide/computation.html"),
                        );
                    }
                }

                // XR004 — .sel() with float
                3 if !config.is_disabled("XR004") => {
                    if let Some(node) = query
                        .capture_index_for_name("xr_sel_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        // Suppress when method= or tolerance= is provided
                        if has_keyword_arg(node, source, "method")
                            || has_keyword_arg(node, source, "tolerance")
                            || parser::has_dictionary_splat(node)
                        {
                            continue;
                        }
                        if !reported_sel_calls.insert(node.id()) {
                            continue;
                        }
                        let (line, col) = position(&node);
                        diags.push(
                            Diagnostic::new(
                                "XR004",
                                Severity::Warning,
                                path,
                                line,
                                col,
                                "`.sel()` called with a float literal — floating-point coordinate comparison may silently return no data",
                            )
                            .with_suggestion("Add `method='nearest'` or `tolerance=1e-6` to handle float imprecision")
                            .with_url("https://docs.xarray.dev/en/stable/generated/xarray.Dataset.sel.html"),
                        );
                    }
                }

                // XR005 — .compute() in loop
                4 if !config.is_disabled("XR005") => {
                    if let Some(node) = query
                        .capture_index_for_name("xr_compute_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        if !is_inside_loop(node) {
                            continue;
                        }
                        let (line, col) = position(&node);
                        diags.push(
                            Diagnostic::new(
                                "XR005",
                                Severity::Error,
                                path,
                                line,
                                col,
                                "`.compute()` called inside a for loop — the full dask task graph is rebuilt and executed on every iteration",
                            )
                            .with_suggestion("Call `.persist()` before the loop, or restructure using `xr.apply_ufunc` / dask.delayed")
                            .with_url("https://docs.dask.org/en/stable/best-practices.html#avoid-calling-compute-repeatedly"),
                        );
                    }
                }

                // XR006 — .to_array() / .to_dataarray() without dim=
                5 if !config.is_disabled("XR006") => {
                    if let Some(call_node) = query
                        .capture_index_for_name("xr_to_array_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        if keyword_arg_present_or_unknown(call_node, source, "dim") {
                            continue;
                        }
                        let (line, col) = position(&call_node);
                        let method = query
                            .capture_index_for_name("xr_to_array_attr")
                            .and_then(|i| m.nodes_for_capture_index(i).next())
                            .map(|n| node_text(&n, source))
                            .unwrap_or("to_array");
                        diags.push(
                            Diagnostic::new(
                                "XR006",
                                Severity::Warning,
                                path,
                                line,
                                col,
                                format!("`.{method}()` called without `dim=` — creates an unnamed 'variable' dimension, making downstream indexing fragile"),
                            )
                            .with_suggestion("Add `dim='variable'` (or a descriptive name) to make the new dimension explicit")
                            .with_fix_hint(format!(".{method}(dim=\"variable\")"))
                            .with_url("https://docs.xarray.dev/en/stable/generated/xarray.Dataset.to_array.html"),
                        );
                    }
                }

                // XR007 — xr.concat in a for loop
                6 if !config.is_disabled("XR007") => {
                    if let Some(node) = query
                        .capture_index_for_name("xr_concat_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        // `pd.concat(...)` and `df.concat(...)` are not xarray;
                        // NP002 owns the pandas/numpy case.
                        if !call_is_from(node, source, &file.imports, "xarray") {
                            continue;
                        }
                        if !is_inside_loop(node) {
                            continue;
                        }
                        let (line, col) = position(&node);
                        diags.push(
                            Diagnostic::new(
                                "XR007",
                                Severity::Error,
                                path,
                                line,
                                col,
                                "`xr.concat()` inside a for loop creates O(n²) intermediate copies",
                            )
                            .with_suggestion("Collect DataArrays/Datasets in a list, then call `xr.concat(items, dim=...)` once outside the loop")
                            .with_url("https://docs.xarray.dev/en/stable/generated/xarray.concat.html"),
                        );
                    }
                }

                // XR008 — open_mfdataset without parallel=True
                7 if !config.is_disabled("XR008") => {
                    if let Some(call_node) = query
                        .capture_index_for_name("xr_mfdataset_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        // Only fires for open_mfdataset, not open_dataset
                        let fn_name = query
                            .capture_index_for_name("xr_mfdataset_attr")
                            .and_then(|i| m.nodes_for_capture_index(i).next())
                            .or_else(|| {
                                query
                                    .capture_index_for_name("xr_mfdataset_bare")
                                    .and_then(|i| m.nodes_for_capture_index(i).next())
                            })
                            .map(|n| node_text(&n, source))
                            .unwrap_or("");
                        if fn_name != "open_mfdataset" {
                            continue;
                        }
                        // Check that parallel= is absent or not True
                        let parallel_val = keyword_arg_value(call_node, source, "parallel");
                        let already_parallel = parallel_val.map(|v| v == "True").unwrap_or(false);
                        if already_parallel || parser::has_dictionary_splat(call_node) {
                            continue;
                        }
                        let (line, col) = position(&call_node);
                        diags.push(
                            Diagnostic::new(
                                "XR008",
                                Severity::Warning,
                                path,
                                line,
                                col,
                                "`open_mfdataset()` called without `parallel=True` — files are opened serially, which can be 10-100× slower on large ensembles",
                            )
                            .with_suggestion("Add `parallel=True` to open files concurrently using `dask.delayed`")
                            .with_fix_hint("open_mfdataset(paths, parallel=True, chunks=\"auto\")")
                            .with_url("https://docs.xarray.dev/en/stable/generated/xarray.open_mfdataset.html"),
                        );
                    }
                }

                // XR009 — apply_ufunc with dask="allowed"
                8 if !config.is_disabled("XR009") => {
                    if let Some(call_node) = query
                        .capture_index_for_name("xr_apply_ufunc_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        // Only fire when dask= kwarg is explicitly "allowed"
                        let dask_val = keyword_arg_value(call_node, source, "dask");
                        let is_allowed = dask_val
                            .map(|v| {
                                let trimmed = v.trim_matches('"').trim_matches('\'');
                                trimmed == "allowed"
                            })
                            .unwrap_or(false);
                        if !is_allowed {
                            continue;
                        }
                        let (line, col) = position(&call_node);
                        diags.push(
                            Diagnostic::new(
                                "XR009",
                                Severity::Warning,
                                path,
                                line,
                                col,
                                "`apply_ufunc(..., dask='allowed')` silently falls back to serial NumPy execution on dask arrays; use `dask='parallelized'` for correct distributed operation",
                            )
                            .with_suggestion("Replace `dask='allowed'` with `dask='parallelized'` and specify `output_dtypes=[...]`")
                            .with_fix_hint("apply_ufunc(func, *args, dask=\"parallelized\", output_dtypes=[float])")
                            .with_url("https://docs.xarray.dev/en/stable/generated/xarray.apply_ufunc.html"),
                        );
                    }
                }

                // XR010 — xr.merge in a for loop
                9 if !config.is_disabled("XR010") => {
                    if let Some(node) = query
                        .capture_index_for_name("xr_merge_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        // `pd.merge(...)` / `df.merge(...)` are pandas, not xarray.
                        if !call_is_from(node, source, &file.imports, "xarray") {
                            continue;
                        }
                        if !is_inside_loop(node) {
                            continue;
                        }
                        let (line, col) = position(&node);
                        diags.push(
                            Diagnostic::new(
                                "XR010",
                                Severity::Warning,
                                path,
                                line,
                                col,
                                "`xr.merge()` inside a for loop — alignment and broadcasting cost is paid on every iteration",
                            )
                            .with_suggestion("Collect datasets in a list, then call `xr.merge(datasets)` once outside the loop")
                            .with_url("https://docs.xarray.dev/en/stable/generated/xarray.merge.html"),
                        );
                    }
                }

                // XR011 — to_netcdf without encoding=
                10 if !config.is_disabled("XR011") => {
                    if let Some(call_node) = query
                        .capture_index_for_name("xr_to_netcdf_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        if keyword_arg_present_or_unknown(call_node, source, "encoding") {
                            continue;
                        }
                        let (line, col) = position(&call_node);
                        diags.push(
                            Diagnostic::new(
                                "XR011",
                                Severity::Hint,
                                path,
                                line,
                                col,
                                "`.to_netcdf()` called without `encoding=` — variables are written as float64 with no compression, potentially 5-10× larger than necessary",
                            )
                            .with_suggestion("Add `encoding={var: {\"dtype\": \"float32\", \"zlib\": True, \"complevel\": 4}}` per variable")
                            .with_url("https://docs.xarray.dev/en/stable/user-guide/io.html#writing-encoded-data"),
                        );
                    }
                }

                _ => {}
            }
        }

        diags
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_source;

    /// Rule IDs fired by `src`, in line order.
    ///
    /// Calls `XarrayRules::check` directly rather than `rules::run_all`, so a
    /// failure points at this rule set alone — import gating and the
    /// cross-domain redundancy filter are deliberately not in the way.
    fn ids(src: &str) -> Vec<&'static str> {
        let parsed = parse_source(src.to_string()).unwrap();
        let mut diags = XarrayRules::check(&parsed, "<test>", &Config::default());
        diags.sort_by_key(|d| (d.line, d.rule_id));
        diags.into_iter().map(|d| d.rule_id).collect()
    }

    fn fires(rule: &'static str, src: &str) -> bool {
        ids(src).contains(&rule)
    }

    const IMPORTS: &str = "import xarray as xr\n";

    #[test]
    fn xr001_open_without_chunks() {
        assert!(fires(
            "XR001",
            &format!("{IMPORTS}ds = xr.open_dataset('a.nc')\n")
        ));
        assert!(!fires(
            "XR001",
            &format!("{IMPORTS}ds = xr.open_dataset('a.nc', chunks='auto')\n")
        ));
    }

    #[test]
    fn xr002_values_property_but_not_method() {
        assert!(fires(
            "XR002",
            &format!("{IMPORTS}ds = xr.open_dataset('a.nc', chunks='auto')\nv = ds.values\n")
        ));
        assert!(!fires(
            "XR002",
            &format!("{IMPORTS}d = {{}}\nfor k in d.values():\n    pass\n")
        ));
    }

    #[test]
    fn xr003_dimension_names_only() {
        assert!(fires(
            "XR003",
            &format!(
                "{IMPORTS}ds = xr.open_dataset('a.nc', chunks='auto')\nfor t in ds.time:\n    pass\n"
            )
        ));
        // An ordinary attribute is not a dimension.
        assert!(!fires(
            "XR003",
            &format!("{IMPORTS}for f in self.files:\n    pass\n")
        ));
    }

    #[test]
    fn xr004_float_coordinates_including_negative() {
        assert!(fires("XR004", &format!("{IMPORTS}x = ds.sel(lat=33.5)\n")));
        assert!(fires("XR004", &format!("{IMPORTS}x = ds.sel(lat=-33.5)\n")));
        assert!(!fires(
            "XR004",
            &format!("{IMPORTS}x = ds.sel(time='2020')\n")
        ));
    }

    #[test]
    fn xr005_compute_inside_a_loop_only() {
        assert!(fires(
            "XR005",
            &format!("{IMPORTS}for p in parts:\n    ds.sel(t=p).compute()\n")
        ));
        assert!(!fires("XR005", &format!("{IMPORTS}ds.compute()\n")));
    }

    #[test]
    fn xr006_to_array_without_dim() {
        assert!(fires("XR006", &format!("{IMPORTS}a = ds.to_array()\n")));
        assert!(!fires(
            "XR006",
            &format!("{IMPORTS}a = ds.to_array(dim='v')\n")
        ));
    }

    #[test]
    fn xr007_concat_in_loop_is_xarray_only() {
        assert!(fires(
            "XR007",
            &format!("{IMPORTS}for f in files:\n    out = xr.concat([out, f], dim='time')\n")
        ));
        // pandas concat is NP002's business, not XR007's.
        assert!(!fires(
            "XR007",
            &format!(
                "{IMPORTS}import pandas as pd\nfor f in files:\n    out = pd.concat([out, f])\n"
            )
        ));
    }

    #[test]
    fn xr008_open_mfdataset_without_parallel() {
        assert!(fires(
            "XR008",
            &format!("{IMPORTS}ds = xr.open_mfdataset('*.nc', chunks='auto')\n")
        ));
        assert!(!fires(
            "XR008",
            &format!("{IMPORTS}ds = xr.open_mfdataset('*.nc', chunks='auto', parallel=True)\n")
        ));
    }

    #[test]
    fn xr009_apply_ufunc_dask_allowed() {
        assert!(fires(
            "XR009",
            &format!("{IMPORTS}r = xr.apply_ufunc(f, ds, dask='allowed')\n")
        ));
        assert!(!fires(
            "XR009",
            &format!("{IMPORTS}r = xr.apply_ufunc(f, ds, dask='parallelized')\n")
        ));
        assert!(!fires(
            "XR009",
            &format!("{IMPORTS}r = xr.apply_ufunc(f, ds)\n")
        ));
    }

    #[test]
    fn xr010_merge_in_loop_is_xarray_only() {
        assert!(fires(
            "XR010",
            &format!("{IMPORTS}for f in files:\n    out = xr.merge([out, f])\n")
        ));
        assert!(!fires(
            "XR010",
            &format!("{IMPORTS}out = xr.merge(items)\n")
        ));
    }

    #[test]
    fn xr011_to_netcdf_without_encoding() {
        assert!(fires("XR011", &format!("{IMPORTS}ds.to_netcdf('o.nc')\n")));
        assert!(!fires(
            "XR011",
            &format!("{IMPORTS}ds.to_netcdf('o.nc', encoding=enc)\n")
        ));
    }

    #[test]
    fn kwargs_splat_silences_missing_keyword_rules() {
        // The keyword may well be in **opts, so a "missing keyword" rule
        // cannot claim it is absent.
        assert!(!fires(
            "XR001",
            &format!("{IMPORTS}ds = xr.open_dataset('a.nc', **opts)\n")
        ));
        assert!(!fires(
            "XR011",
            &format!("{IMPORTS}ds.to_netcdf('o.nc', **opts)\n")
        ));
    }

    #[test]
    fn disabled_rules_do_not_fire() {
        let src = format!("{IMPORTS}ds = xr.open_dataset('a.nc')\n");
        let parsed = parse_source(src).unwrap();
        let mut config = Config::default();
        config.disable.insert("XR001".to_string());
        let diags = XarrayRules::check(&parsed, "<test>", &config);
        assert!(diags.iter().all(|d| d.rule_id != "XR001"));
    }
}
