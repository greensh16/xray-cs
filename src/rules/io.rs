use std::sync::LazyLock;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Query, QueryCursor};

use crate::{
    config::Config,
    diagnostic::{Diagnostic, RuleMeta, Severity},
    parser::{
        ParsedFile, call_is_from, is_inside_loop, keyword_arg_present_or_unknown,
        keyword_arg_value, node_text, position,
    },
};

use super::RuleSet;

pub struct IoRules;

const QUERY_SRC: &str = include_str!("../../queries/io.scm");

/// Compiled once per process and shared across all rayon workers.
/// `Query` is `Send + Sync`; only the `QueryCursor` needs to be per-call.
/// A compilation failure is a bug in xray itself, so we fail loudly.
static QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&tree_sitter_python::LANGUAGE.into(), QUERY_SRC)
        .unwrap_or_else(|e| panic!("xray: BUG — failed to compile io query: {e}"))
});

impl RuleSet for IoRules {
    fn meta() -> Vec<RuleMeta> {
        vec![
            RuleMeta {
                id: "IO001",
                name: "np-save-large-arrays",
                severity: Severity::Hint,
                description: "np.save() used — uncompressed, unchunked; prefer Zarr or HDF5 for large arrays",
            },
            RuleMeta {
                id: "IO002",
                name: "netcdf4-direct-open",
                severity: Severity::Hint,
                description: "netCDF4.Dataset opened directly — bypasses xarray coordinate alignment machinery",
            },
            RuleMeta {
                id: "IO003",
                name: "zarr-open-without-chunks",
                severity: Severity::Warning,
                description: "zarr.open called without chunks= — unchunked Zarr defeats compression and parallel I/O",
            },
            RuleMeta {
                id: "IO004",
                name: "netcdf4-read-in-loop",
                severity: Severity::Warning,
                description: "netCDF4 variable subscripted inside a loop — each read may hit disk; pre-load outside the loop",
            },
            RuleMeta {
                id: "IO005",
                name: "h5py-file-without-swmr",
                severity: Severity::Hint,
                description: "h5py.File opened without swmr=True — consider SWMR mode for concurrent HPC read workflows",
            },
            RuleMeta {
                id: "IO006",
                name: "open-dataset-scipy-engine",
                severity: Severity::Warning,
                description: "xr.open_dataset called with engine='scipy' — loads eagerly without chunking; use 'netcdf4' or 'zarr'",
            },
        ]
    }

    fn check(file: &ParsedFile, path: &str, config: &Config) -> Vec<Diagnostic> {
        let mut diags = Vec::new();
        let source = file.source.as_bytes();
        let query = &*QUERY;

        let mut cursor = QueryCursor::new();
        let root = file.tree.root_node();

        // Names bound to a netCDF4 Dataset or one of its variables (IO004).
        let nc_handles = netcdf_handles(file, source);

        let mut matches = cursor.matches(query, root, source);
        while let Some(m) = matches.next() {
            match m.pattern_index {
                // IO001 — np.save
                0 if !config.is_disabled("IO001") && config.io.flag_missing_compression => {
                    if let Some(node) = query
                        .capture_index_for_name("io_npsave_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        // Only fire for numpy's save(), resolved through the
                        // file's import aliases rather than a hard-coded `np`.
                        if !call_is_from(node, source, &file.imports, "numpy") {
                            continue;
                        }
                        let (line, col) = position(&node);
                        diags.push(
                            Diagnostic::new(
                                "IO001",
                                Severity::Hint,
                                path,
                                line,
                                col,
                                "`np.save()` stores arrays uncompressed and unchunked — poor for large HPC datasets",
                            )
                            .with_suggestion("Use `zarr.save(path, arr, chunks=(...), compressor=Blosc())` or `h5py` for large scientific arrays")
                            .with_url("https://zarr.readthedocs.io/en/stable/"),
                        );
                    }
                }

                // IO002 — netCDF4.Dataset direct open
                1 if !config.is_disabled("IO002") && config.io.flag_missing_compression => {
                    if let Some(node) = query
                        .capture_index_for_name("io_nc4_dataset_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        let (line, col) = position(&node);
                        diags.push(
                            Diagnostic::new(
                                "IO002",
                                Severity::Hint,
                                path,
                                line,
                                col,
                                "`netCDF4.Dataset()` bypasses xarray's coordinate alignment, CF metadata, and lazy loading",
                            )
                            .with_suggestion("Use `xr.open_dataset(path, chunks='auto')` unless you specifically need the low-level netCDF4 API")
                            .with_url("https://docs.xarray.dev/en/stable/generated/xarray.open_dataset.html"),
                        );
                    }
                }

                // IO003 — zarr.open without chunks
                // Use AST-based keyword argument check to avoid substring false positives
                // (e.g. a string argument that happens to contain "chunks").
                2 if !config.is_disabled("IO003") => {
                    if let Some(call_node) = query
                        .capture_index_for_name("io_zarr_open_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        // Only fire for zarr's open*(). A bare `open(...)` is
                        // the builtin unless it was explicitly imported from
                        // zarr — treating every bare open() as zarr flagged
                        // `with open("notes.txt") as fh:` in any file that
                        // imported zarr.
                        if !call_is_from(call_node, source, &file.imports, "zarr") {
                            continue;
                        }
                        if !keyword_arg_present_or_unknown(call_node, source, "chunks") {
                            let (line, col) = position(&call_node);
                            diags.push(
                                Diagnostic::new(
                                    "IO003",
                                    Severity::Warning,
                                    path,
                                    line,
                                    col,
                                    "`zarr.open()` called without `chunks=` — the array is stored as a single chunk, disabling parallel I/O",
                                )
                                .with_suggestion("Set `chunks` to match your access pattern, e.g. `chunks=(time, 256, 256)` for time-series grids")
                                .with_url("https://zarr.readthedocs.io/en/stable/tutorial.html#chunk-optimizations"),
                            );
                        }
                    }
                }

                // IO004 — netCDF4 variable read in loop
                3 if !config.is_disabled("IO004") => {
                    if let Some(node) = query
                        .capture_index_for_name("io_nc_subscript")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        // The query matches any `name[...]`, so without this
                        // check every list index and dict lookup in every loop
                        // was reported.  Only subscripts of a name that was
                        // bound from a netCDF4 handle count.
                        if !file.imports.netcdf4 || !is_inside_loop(node) {
                            continue;
                        }
                        let var_name = query
                            .capture_index_for_name("io_nc_var")
                            .and_then(|i| m.nodes_for_capture_index(i).next())
                            .map(|n| node_text(&n, source))
                            .unwrap_or("");
                        if !nc_handles.contains(var_name) {
                            continue;
                        }
                        let (line, col) = position(&node);
                        diags.push(
                            Diagnostic::new(
                                "IO004",
                                Severity::Warning,
                                path,
                                line,
                                col,
                                format!(
                                    "netCDF4 variable `{var_name}` subscripted inside a loop — each read may trigger a disk seek"
                                ),
                            )
                            .with_suggestion("Pre-load the full array outside the loop with `data = nc_var[:]`, then index `data[i]`")
                            .with_url("https://github.com/greensh16/xray/wiki/IO-Rules#io004"),
                        );
                    }
                }

                // IO005 — h5py.File without swmr=True
                4 if !config.is_disabled("IO005") => {
                    if let Some(call_node) = query
                        .capture_index_for_name("io_h5py_file_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        // Only fire for h5py's File(), resolved through imports.
                        if !call_is_from(call_node, source, &file.imports, "h5py") {
                            continue;
                        }
                        if !keyword_arg_present_or_unknown(call_node, source, "swmr") {
                            let (line, col) = position(&call_node);
                            diags.push(
                                Diagnostic::new(
                                    "IO005",
                                    Severity::Hint,
                                    path,
                                    line,
                                    col,
                                    "`h5py.File()` opened without `swmr=True` — concurrent reads in an HPC job may return stale data",
                                )
                                .with_suggestion("Add `swmr=True` when the file will be read concurrently by multiple processes")
                                .with_url("https://docs.h5py.org/en/stable/swmr.html"),
                            );
                        }
                    }
                }

                // IO006 — xr.open_dataset with engine="scipy"
                5 if !config.is_disabled("IO006") => {
                    if let Some(call_node) = query
                        .capture_index_for_name("io_open_scipy_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        // Only fire when engine= is explicitly set to "scipy"
                        let engine = keyword_arg_value(call_node, source, "engine");
                        if engine.map(|v| v.contains("scipy")).unwrap_or(false) {
                            let (line, col) = position(&call_node);
                            diags.push(
                                Diagnostic::new(
                                    "IO006",
                                    Severity::Warning,
                                    path,
                                    line,
                                    col,
                                    "`engine='scipy'` loads the entire file eagerly — no chunking, no lazy access, poor for large HPC datasets",
                                )
                                .with_suggestion("Use `engine='netcdf4'` for standard NetCDF files, or `engine='zarr'` for chunked cloud-native storage")
                                .with_fix_hint("engine=\"netcdf4\"")
                                .with_url("https://docs.xarray.dev/en/stable/generated/xarray.open_dataset.html"),
                            );
                        }
                    }
                }

                _ => {}
            }
        }

        diags
    }
}

/// Collect local names that hold a netCDF4 `Dataset` or one of its variables.
///
/// A light single-pass approximation of dataflow: an assignment counts when its
/// right-hand side calls netCDF4's `Dataset(...)`, or reads `.variables[...]`
/// / `.createVariable(...)` off something already known to be a handle.  Good
/// enough to distinguish `temps = nc.variables["t2m"]` from an ordinary list,
/// which is all IO004 needs.
fn netcdf_handles(file: &ParsedFile, source: &[u8]) -> std::collections::HashSet<String> {
    let mut handles: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut stack = vec![file.tree.root_node()];

    while let Some(node) = stack.pop() {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }

        if node.kind() != "assignment" {
            continue;
        }
        let (Some(left), Some(right)) = (
            node.child_by_field_name("left"),
            node.child_by_field_name("right"),
        ) else {
            continue;
        };
        if left.kind() != "identifier" {
            continue;
        }

        let rhs = node_text(&right, source);
        let is_handle = match right.kind() {
            // temps = nc.variables["t2m"]  /  temps = nc["t2m"]
            "subscript" => right
                .child_by_field_name("value")
                .map(|v| {
                    let text = node_text(&v, source);
                    text.ends_with(".variables")
                        || handles.contains(text)
                        || attribute_owner_is_handle(&handles, text)
                })
                .unwrap_or(false),
            // nc = netCDF4.Dataset(path)  /  var = nc.createVariable(...)
            "call" => {
                call_is_from(right, source, &file.imports, "netCDF4")
                    || rhs.contains(".createVariable(")
            }
            // temps = nc.variables
            "attribute" => rhs.ends_with(".variables"),
            _ => false,
        };

        if is_handle {
            handles.insert(node_text(&left, source).to_string());
        }
    }

    handles
}

/// Is `text` an attribute chain rooted at a known netCDF4 handle
/// (`nc.variables`, `ds.groups[...]`, …)?
fn attribute_owner_is_handle(handles: &std::collections::HashSet<String>, text: &str) -> bool {
    text.split('.')
        .next()
        .is_some_and(|root| handles.contains(root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_source;

    /// Rule IDs fired by `src`, in line order.
    fn ids(src: &str) -> Vec<&'static str> {
        let parsed = parse_source(src.to_string()).unwrap();
        let mut diags = IoRules::check(&parsed, "<test>", &Config::default());
        diags.sort_by_key(|d| (d.line, d.rule_id));
        diags.into_iter().map(|d| d.rule_id).collect()
    }

    fn fires(rule: &'static str, src: &str) -> bool {
        ids(src).contains(&rule)
    }

    const IMPORTS: &str =
        "import numpy as np\nimport netCDF4\nimport zarr\nimport h5py\nimport xarray as xr\n";

    #[test]
    fn io001_np_save_is_numpy_only() {
        assert!(fires("IO001", &format!("{IMPORTS}np.save('a.npy', arr)\n")));
        // A same-named method on an unrelated object must not fire.
        assert!(!fires("IO001", &format!("{IMPORTS}model.save('a.pkl')\n")));
    }

    #[test]
    fn io001_and_io002_respect_the_compression_toggle() {
        let src = format!("{IMPORTS}np.save('a.npy', arr)\nds = netCDF4.Dataset('a.nc')\n");
        let parsed = parse_source(src).unwrap();
        let mut config = Config::default();
        config.io.flag_missing_compression = false;
        let diags = IoRules::check(&parsed, "<test>", &config);
        assert!(
            diags
                .iter()
                .all(|d| d.rule_id != "IO001" && d.rule_id != "IO002")
        );
    }

    #[test]
    fn io002_netcdf4_direct_open() {
        assert!(fires(
            "IO002",
            &format!("{IMPORTS}ds = netCDF4.Dataset('a.nc')\n")
        ));
    }

    #[test]
    fn io003_zarr_open_is_not_the_builtin_open() {
        assert!(fires(
            "IO003",
            &format!("{IMPORTS}z = zarr.open('a.zarr')\n")
        ));
        // The builtin open() has nothing to do with zarr.
        assert!(!fires("IO003", &format!("{IMPORTS}f = open('a.txt')\n")));
        assert!(!fires(
            "IO003",
            &format!("{IMPORTS}z = zarr.open('a.zarr', chunks=(10,))\n")
        ));
    }

    #[test]
    fn io004_needs_a_netcdf_handle() {
        // The handle is bound outside the loop and subscripted inside it —
        // each read may hit disk on every iteration.
        assert!(fires(
            "IO004",
            &format!(
                "{IMPORTS}nc = netCDF4.Dataset('a.nc')\ntemp = nc.variables['t']\nfor i in r:\n    v = temp[i]\n"
            )
        ));
        // Plain list and dict indexing inside a loop is not a netCDF read.
        assert!(!fires(
            "IO004",
            &format!("{IMPORTS}rows = [1, 2]\nfor i in r:\n    v = rows[i]\n")
        ));
        assert!(!fires(
            "IO004",
            &format!("{IMPORTS}d = {{}}\nfor i in r:\n    v = d['k']\n")
        ));
    }

    #[test]
    fn io005_h5py_file_without_swmr() {
        assert!(fires(
            "IO005",
            &format!("{IMPORTS}f = h5py.File('a.h5', 'r')\n")
        ));
        assert!(!fires(
            "IO005",
            &format!("{IMPORTS}f = h5py.File('a.h5', 'r', swmr=True)\n")
        ));
        // **kwargs may carry swmr, so the rule cannot claim it is absent.
        assert!(!fires(
            "IO005",
            &format!("{IMPORTS}f = h5py.File('a.h5', 'r', **opts)\n")
        ));
    }

    #[test]
    fn io006_scipy_engine() {
        assert!(fires(
            "IO006",
            &format!("{IMPORTS}ds = xr.open_dataset('a.nc', engine='scipy')\n")
        ));
        assert!(!fires(
            "IO006",
            &format!("{IMPORTS}ds = xr.open_dataset('a.nc', engine='netcdf4')\n")
        ));
    }

    #[test]
    fn disabled_rules_do_not_fire() {
        let parsed = parse_source(format!("{IMPORTS}np.save('a.npy', arr)\n")).unwrap();
        let mut config = Config::default();
        config.disable.insert("IO001".to_string());
        let diags = IoRules::check(&parsed, "<test>", &config);
        assert!(diags.iter().all(|d| d.rule_id != "IO001"));
    }
}
