use std::sync::LazyLock;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Query, QueryCursor};

use crate::{
    config::Config,
    diagnostic::{Diagnostic, RuleMeta, Severity},
    parser::{ParsedFile, call_is_from, is_inside_loop, keyword_arg_present_or_unknown, position},
};

use super::RuleSet;

pub struct DaskRules;

const QUERY_SRC: &str = include_str!("../../queries/dask.scm");

/// Compiled once per process and shared across all rayon workers.
/// `Query` is `Send + Sync`; only the `QueryCursor` needs to be per-call.
/// A compilation failure is a bug in xray itself, so we fail loudly.
static QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&tree_sitter_python::LANGUAGE.into(), QUERY_SRC)
        .unwrap_or_else(|e| panic!("xray: BUG — failed to compile dask query: {e}"))
});

impl RuleSet for DaskRules {
    fn meta() -> Vec<RuleMeta> {
        vec![
            RuleMeta {
                id: "DK001",
                name: "compute-in-for-loop",
                severity: Severity::Error,
                description: ".compute() called inside a for loop — rebuilds the full task graph every iteration",
            },
            RuleMeta {
                id: "DK002",
                name: "dask-compute-in-for-loop",
                severity: Severity::Error,
                description: "dask.compute() called inside a for loop",
            },
            RuleMeta {
                id: "DK003",
                name: "excessive-compute-calls",
                severity: Severity::Warning,
                description: "More .compute() calls in one file than dask.compute_call_threshold — consider .persist() for reused graphs",
            },
            RuleMeta {
                id: "DK004",
                name: "immediate-compute",
                severity: Severity::Hint,
                description: "Dask operation immediately followed by .compute() — no lazy benefit, use pandas/numpy directly",
            },
            RuleMeta {
                id: "DK005",
                name: "persist-result-discarded",
                severity: Severity::Warning,
                description: ".persist() result not assigned — cost of materialising the graph is paid with no benefit",
            },
            RuleMeta {
                id: "DK006",
                name: "persist-then-compute",
                severity: Severity::Warning,
                description: ".persist().compute() chain — persist() is redundant; just call .compute() directly",
            },
            RuleMeta {
                id: "DK007",
                name: "from-array-without-chunks",
                severity: Severity::Warning,
                description: "da.from_array() called without chunks= — creates a single-chunk array that defeats dask parallelism",
            },
            RuleMeta {
                id: "DK008",
                name: "rechunk-in-loop",
                severity: Severity::Warning,
                description: ".rechunk() called inside a for loop — triggers a full graph materialisation on every iteration",
            },
            RuleMeta {
                id: "DK009",
                name: "concatenate-in-loop",
                severity: Severity::Error,
                description: "da.concatenate() inside a for loop — O(n²) intermediate copies; collect arrays then concatenate once",
            },
        ]
    }

    fn check(file: &ParsedFile, path: &str, config: &Config) -> Vec<Diagnostic> {
        let mut diags = Vec::new();
        let source = file.source.as_bytes();
        let query = &*QUERY;

        let mut cursor = QueryCursor::new();
        let root = file.tree.root_node();

        // Count all .compute() calls for DK003
        let mut compute_call_count = 0usize;
        let mut compute_call_positions = Vec::new();

        let mut matches = cursor.matches(query, root, source);
        while let Some(m) = matches.next() {
            match m.pattern_index {
                // DK001 — .compute() in for loop
                0 if !config.is_disabled("DK001") => {
                    if let Some(node) = query
                        .capture_index_for_name("dk_compute_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        if !is_inside_loop(node) {
                            continue;
                        }
                        let (line, col) = position(&node);
                        diags.push(
                            Diagnostic::new(
                                "DK001",
                                Severity::Error,
                                path,
                                line,
                                col,
                                "`.compute()` inside a for loop materialises the full dask graph on every iteration",
                            )
                            .with_suggestion("Call `.persist()` before the loop to keep the result in distributed memory")
                            .with_url("https://docs.dask.org/en/stable/best-practices.html"),
                        );
                    }
                }

                // DK002 — dask.compute() in for loop
                1 if !config.is_disabled("DK002") => {
                    if let Some(node) = query
                        .capture_index_for_name("dk_dask_compute_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        if !is_inside_loop(node) {
                            continue;
                        }
                        let (line, col) = position(&node);
                        diags.push(
                            Diagnostic::new(
                                "DK002",
                                Severity::Error,
                                path,
                                line,
                                col,
                                "`dask.compute()` called inside a for loop — consider batching all delayed objects and computing once",
                            )
                            .with_suggestion("Collect delayed objects in a list, then call `dask.compute(*items)` outside the loop")
.with_url("https://github.com/greensh16/xray/wiki/Dask-Rules#DK002"),
                        );
                    }
                }

                // DK003 — collect all .compute() calls
                2 => {
                    if let Some(node) = query
                        .capture_index_for_name("dk_any_compute_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        compute_call_count += 1;
                        compute_call_positions.push(position(&node));
                    }
                }

                // DK004 — immediate .compute() after a call (non-persist)
                3 if !config.is_disabled("DK004") => {
                    if let Some(node) = query
                        .capture_index_for_name("dk_immediate_compute_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        let (line, col) = position(&node);
                        diags.push(
                            Diagnostic::new(
                                "DK004",
                                Severity::Hint,
                                path,
                                line,
                                col,
                                "Dask operation immediately followed by `.compute()` — the lazy graph is never reused",
                            )
                            .with_suggestion("If you never reuse this result lazily, consider using pandas/numpy directly")
                            .with_url("https://docs.dask.org/en/stable/best-practices.html#avoid-calling-compute-repeatedly"),
                        );
                    }
                }

                // DK005 — .persist() result discarded
                4 if !config.is_disabled("DK005") => {
                    if let Some(node) = query
                        .capture_index_for_name("dk_persist_uncaptured")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        let (line, col) = position(&node);
                        diags.push(
                            Diagnostic::new(
                                "DK005",
                                Severity::Warning,
                                path,
                                line,
                                col,
                                "`.persist()` result not assigned — the materialised graph is immediately discarded",
                            )
                            .with_suggestion("Assign the result: `hot = arr.persist()`, then reuse `hot` across multiple operations")
                            .with_url("https://docs.dask.org/en/stable/api.html#dask.persist"),
                        );
                    }
                }

                // DK006 — .persist().compute() chain
                5 if !config.is_disabled("DK006") => {
                    if let Some(node) = query
                        .capture_index_for_name("dk_persist_then_compute")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        let (line, col) = position(&node);
                        diags.push(
                            Diagnostic::new(
                                "DK006",
                                Severity::Warning,
                                path,
                                line,
                                col,
                                "`.persist().compute()` chain — `persist()` distributes work in the cluster but `.compute()` immediately pulls it back to local memory, negating the benefit",
                            )
                            .with_suggestion("Use `.compute()` alone, or `.persist()` without `.compute()` if you need the result to remain distributed")
                            .with_url("https://docs.dask.org/en/stable/best-practices.html"),
                        );
                    }
                }

                // DK007 — da.from_array() without chunks=
                6 if !config.is_disabled("DK007") => {
                    if let Some(call_node) = query
                        .capture_index_for_name("dk_from_array_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        // Only fire for dask's from_array, resolved through the
                        // file's import aliases — the old hard-coded `da`/`dask`
                        // receiver check missed `import dask.array as dsa`.
                        if !call_is_from(call_node, source, &file.imports, "dask") {
                            continue;
                        }
                        if !keyword_arg_present_or_unknown(call_node, source, "chunks") {
                            let (line, col) = position(&call_node);
                            diags.push(
                                Diagnostic::new(
                                    "DK007",
                                    Severity::Warning,
                                    path,
                                    line,
                                    col,
                                    "`da.from_array()` called without `chunks=` — creates a single monolithic chunk; no parallelism is possible",
                                )
                                .with_suggestion("Add `chunks=` matching your array shape, e.g. `chunks=(1000, 1000)` or `chunks='auto'`")
                                .with_fix_hint("da.from_array(arr, chunks=\"auto\")")
                                .with_url("https://docs.dask.org/en/stable/array-creation.html"),
                            );
                        }
                    }
                }

                // DK008 — .rechunk() in a for loop
                7 if !config.is_disabled("DK008") => {
                    if let Some(node) = query
                        .capture_index_for_name("dk_rechunk_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        if !is_inside_loop(node) {
                            continue;
                        }
                        let (line, col) = position(&node);
                        diags.push(
                            Diagnostic::new(
                                "DK008",
                                Severity::Warning,
                                path,
                                line,
                                col,
                                "`.rechunk()` inside a for loop materialises and re-partitions the array on every iteration",
                            )
                            .with_suggestion("Call `.rechunk(target_chunks)` once before the loop with the desired chunk layout")
                            .with_url("https://docs.dask.org/en/stable/array-best-practices.html#rechunking"),
                        );
                    }
                }

                // DK009 — da.concatenate() in a for loop
                8 if !config.is_disabled("DK009") => {
                    if let Some(node) = query
                        .capture_index_for_name("dk_concatenate_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        if !is_inside_loop(node) {
                            continue;
                        }
                        // Only fire for dask's concatenate — not np.concatenate
                        // (NP002) or xr.concat (XR007).
                        if !call_is_from(node, source, &file.imports, "dask") {
                            continue;
                        }
                        let (line, col) = position(&node);
                        diags.push(
                            Diagnostic::new(
                                "DK009",
                                Severity::Error,
                                path,
                                line,
                                col,
                                "`da.concatenate()` inside a for loop creates O(n²) intermediate copies — each iteration copies all previously concatenated data",
                            )
                            .with_suggestion("Collect arrays in a list, then call `da.concatenate(arrays, axis=0)` once outside the loop")
                            .with_url("https://docs.dask.org/en/stable/array-api.html#dask.array.concatenate"),
                        );
                    }
                }

                _ => {}
            }
        }

        // DK003 — fire once the file has more computes than the threshold
        // allows.  Both xray.toml and `xray init` describe the threshold as the
        // number of calls tolerated *before* the rule fires, so the comparison
        // is strictly greater-than.
        if !config.is_disabled("DK003") && compute_call_count > config.dask.compute_call_threshold {
            // Report on the first call past the threshold
            if let Some(&(line, col)) =
                compute_call_positions.get(config.dask.compute_call_threshold)
            {
                diags.push(
                    Diagnostic::new(
                        "DK003",
                        Severity::Warning,
                        path,
                        line,
                        col,
                        format!(
                            "{} `.compute()` calls in this file — intermediate results may benefit from `.persist()`",
                            compute_call_count
                        ),
                    )
                    .with_suggestion("Use `result = computation.persist()` to keep hot data in distributed memory across calls")
                    .with_url("https://docs.dask.org/en/stable/api.html#dask.persist"),
                );
            }
        }

        diags
    }
}
