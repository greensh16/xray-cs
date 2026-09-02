use std::sync::LazyLock;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Query, QueryCursor};

use crate::{
    config::Config,
    diagnostic::{Diagnostic, RuleMeta, Severity},
    parser::{
        ParsedFile, call_module, is_inside_loop, keyword_arg_present_or_unknown, node_text,
        position,
    },
};

use super::RuleSet;

pub struct NumpyRules;

const QUERY_SRC: &str = include_str!("../../queries/numpy.scm");

/// Compiled once per process and shared across all rayon workers.
/// `Query` is `Send + Sync`; only the `QueryCursor` needs to be per-call.
/// A compilation failure is a bug in xray itself, so we fail loudly.
static QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&tree_sitter_python::LANGUAGE.into(), QUERY_SRC)
        .unwrap_or_else(|e| panic!("xray: BUG — failed to compile numpy query: {e}"))
});

impl RuleSet for NumpyRules {
    fn meta() -> Vec<RuleMeta> {
        vec![
            RuleMeta {
                id: "NP001",
                name: "iterrows",
                severity: Severity::Warning,
                description: "DataFrame.iterrows() — row-by-row Python iteration, use vectorised operations",
            },
            RuleMeta {
                id: "NP002",
                name: "concat-in-loop",
                severity: Severity::Error,
                description: "pd.concat / np.concatenate inside a loop — quadratic copy overhead",
            },
            RuleMeta {
                id: "NP003",
                name: "alloc-without-dtype",
                severity: Severity::Hint,
                description: "np.zeros/ones/empty called without dtype= — silently defaults to float64",
            },
            RuleMeta {
                id: "NP004",
                name: "math-scalar-fn",
                severity: Severity::Warning,
                description: "math.* scalar function — replace with numpy ufunc; Warning in loops, Hint elsewhere",
            },
            RuleMeta {
                id: "NP005",
                name: "chained-indexing",
                severity: Severity::Warning,
                description: "Chained indexing df[col][row] — creates a copy; assignments silently fail",
            },
            RuleMeta {
                id: "NP006",
                name: "matrix-deprecated",
                severity: Severity::Warning,
                description: "np.matrix() is deprecated since NumPy 1.16 — use np.array() / np.ndarray instead",
            },
            RuleMeta {
                id: "NP007",
                name: "applymap-or-apply-lambda-in-loop",
                severity: Severity::Warning,
                description: "DataFrame.applymap() is deprecated (use .map()), or .apply(lambda) inside a loop",
            },
        ]
    }

    fn check(file: &ParsedFile, path: &str, config: &Config) -> Vec<Diagnostic> {
        let mut diags = Vec::new();
        let source = file.source.as_bytes();
        let query = &*QUERY;

        let mut cursor = QueryCursor::new();
        let root = file.tree.root_node();

        let mut matches = cursor.matches(query, root, source);
        while let Some(m) = matches.next() {
            match m.pattern_index {
                // NP001 — iterrows
                0 if !config.is_disabled("NP001") && config.numpy.flag_iterrows => {
                    if let Some(node) = query
                        .capture_index_for_name("np_iterrows_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        let (line, col) = position(&node);
                        diags.push(
                            Diagnostic::new(
                                "NP001",
                                Severity::Warning,
                                path,
                                line,
                                col,
                                "`.iterrows()` iterates row-by-row in Python — typically 10-100× slower than vectorised alternatives",
                            )
                            .with_suggestion("Use `df.apply()`, `df['col'].map()`, or NumPy operations on the underlying arrays")
                            .with_url("https://pandas.pydata.org/docs/user_guide/enhancingperf.html"),
                        );
                    }
                }

                // NP002 — concat in loop
                1 if !config.is_disabled("NP002") => {
                    if let Some(node) = query
                        .capture_index_for_name("np_concat_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        if !is_inside_loop(node) {
                            continue;
                        }
                        // Only fire for numpy/pandas calls, not xr.concat etc.
                        // Resolved through import aliases so `import pandas as
                        // pandas_lib` works and `df.concat(...)` does not match.
                        if !matches!(
                            call_module(node, source, &file.imports),
                            Some("numpy") | Some("pandas")
                        ) {
                            continue;
                        }
                        let (line, col) = position(&node);
                        let fn_name = query
                            .capture_index_for_name("np_concat_method")
                            .and_then(|i| m.nodes_for_capture_index(i).next())
                            .or_else(|| {
                                query
                                    .capture_index_for_name("np_concat_bare")
                                    .and_then(|i| m.nodes_for_capture_index(i).next())
                            })
                            .map(|n| node_text(&n, source))
                            .unwrap_or("concat");
                        diags.push(
                            Diagnostic::new(
                                "NP002",
                                Severity::Error,
                                path,
                                line,
                                col,
                                format!("`{fn_name}()` inside a loop creates O(n²) intermediate copies"),
                            )
                            .with_suggestion("Collect arrays in a list outside the loop, then call `np.concatenate(parts)` once")
                            .with_url("https://github.com/greensh16/xray/wiki/NumPy-Pandas-Rules#np002"),
                        );
                    }
                }

                // NP003 — alloc without dtype
                // Use AST-based keyword argument check instead of substring matching
                // to avoid false positives from comments or string args containing "dtype".
                2 if !config.is_disabled("NP003") => {
                    if let Some(call_node) = query
                        .capture_index_for_name("np_alloc_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        // Belt-and-suspenders: verify the function attribute is actually one of
                        // the target allocators. tree-sitter 0.26 may return structural matches
                        // before predicate filtering when alternatives ([...]) are involved.
                        let fn_name_raw = call_node
                            .child_by_field_name("function")
                            .and_then(|f| {
                                if f.kind() == "attribute" {
                                    f.child_by_field_name("attribute")
                                } else {
                                    Some(f) // bare identifier
                                }
                            })
                            .map(|n| node_text(&n, source))
                            .unwrap_or("");
                        if !matches!(fn_name_raw, "zeros" | "ones" | "empty" | "full") {
                            continue;
                        }
                        // Only fire for numpy calls, not da.ones() etc.
                        if call_module(call_node, source, &file.imports) != Some("numpy") {
                            continue;
                        }
                        if !keyword_arg_present_or_unknown(call_node, source, "dtype") {
                            let (line, col) = position(&call_node);
                            let fn_name = query
                                .capture_index_for_name("np_alloc_method")
                                .and_then(|i| m.nodes_for_capture_index(i).next())
                                .or_else(|| {
                                    query
                                        .capture_index_for_name("np_alloc_bare")
                                        .and_then(|i| m.nodes_for_capture_index(i).next())
                                })
                                .map(|n| node_text(&n, source))
                                .unwrap_or(fn_name_raw);
                            diags.push(
                                Diagnostic::new(
                                    "NP003",
                                    Severity::Hint,
                                    path,
                                    line,
                                    col,
                                    format!("`{fn_name}()` without `dtype=` defaults to float64 — double the memory for integer workloads"),
                                )
                                .with_suggestion("Add `dtype=np.float32` (or int32, int16 etc.) to match your actual data precision")
                                .with_url("https://github.com/greensh16/xray/wiki/NumPy-Pandas-Rules#np003"),
                            );
                        }
                    }
                }

                // NP004 — math.* scalar function
                // Warning when inside a for loop (element-by-element iteration),
                // Hint when called outside a loop (still suboptimal for arrays).
                3 if !config.is_disabled("NP004") => {
                    if let Some(node) = query
                        .capture_index_for_name("np_math_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        let (line, col) = position(&node);
                        let fn_name = query
                            .capture_index_for_name("np_math_fn")
                            .and_then(|i| m.nodes_for_capture_index(i).next())
                            .map(|n| node_text(&n, source))
                            .unwrap_or("fn");
                        let in_loop = is_inside_loop(node);
                        let (severity, message) = if in_loop {
                            (
                                Severity::Warning,
                                format!(
                                    "`math.{fn_name}()` in a loop — scalar math function called element-by-element"
                                ),
                            )
                        } else {
                            (
                                Severity::Hint,
                                format!(
                                    "`math.{fn_name}()` — scalar function; `np.{fn_name}()` operates on whole arrays at once"
                                ),
                            )
                        };
                        diags.push(
                            Diagnostic::new("NP004", severity, path, line, col, message)
                                .with_suggestion(format!("Replace with `np.{fn_name}(array)` to operate on the whole array at once"))
                                .with_fix_hint(format!("np.{fn_name}(array)"))
                                .with_url("https://github.com/greensh16/xray/wiki/NumPy-Pandas-Rules#np004"),
                        );
                    }
                }

                // NP005 — chained indexing
                4 if !config.is_disabled("NP005") => {
                    if let Some(node) = query
                        .capture_index_for_name("np_chained_index")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        // The query matches any `a[..][..]`, which flagged
                        // `grid[0][1]` on a list of lists.  Chained *indexing*
                        // in the pandas sense selects a column by name first,
                        // so require the inner subscript to use a string key.
                        if !inner_subscript_is_string_key(node, source) {
                            continue;
                        }
                        let (line, col) = position(&node);
                        diags.push(
                            Diagnostic::new(
                                "NP005",
                                Severity::Warning,
                                path,
                                line,
                                col,
                                "Chained indexing `df[col][row]` may operate on a copy — assignments here silently don't propagate",
                            )
                            .with_suggestion("Use `df.loc[row, col]` or `df.iloc[row_idx, col_idx]` for safe assignment")
                            .with_url("https://pandas.pydata.org/docs/user_guide/indexing.html#returning-a-view-versus-a-copy"),
                        );
                    }
                }

                // NP006 — np.matrix() deprecated
                5 if !config.is_disabled("NP006") => {
                    if let Some(node) = query
                        .capture_index_for_name("np_matrix_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        // Only fire for numpy's matrix(), resolved through aliases.
                        if call_module(node, source, &file.imports) != Some("numpy") {
                            continue;
                        }
                        let (line, col) = position(&node);
                        diags.push(
                            Diagnostic::new(
                                "NP006",
                                Severity::Warning,
                                path,
                                line,
                                col,
                                "`np.matrix()` is deprecated since NumPy 1.16 and will be removed in a future release",
                            )
                            .with_suggestion("Replace with `np.array(...)` — use `@` for matrix multiplication and `.T` for transpose")
                            .with_fix_hint("np.array(data)")
                            .with_url("https://numpy.org/doc/stable/reference/generated/numpy.matrix.html"),
                        );
                    }
                }

                // NP007a — .applymap() deprecated
                6 if !config.is_disabled("NP007") => {
                    if let Some(node) = query
                        .capture_index_for_name("np_applymap_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        let (line, col) = position(&node);
                        diags.push(
                            Diagnostic::new(
                                "NP007",
                                Severity::Warning,
                                path,
                                line,
                                col,
                                "`.applymap()` is deprecated since pandas 2.1 — it has been renamed to `.map()`",
                            )
                            .with_suggestion("Replace `.applymap(fn)` with `.map(fn)`")
                            .with_fix_hint(".map(fn)")
                            .with_url("https://pandas.pydata.org/docs/reference/api/pandas.DataFrame.map.html"),
                        );
                    }
                }

                // NP007b — .apply(lambda) in a loop
                7 if !config.is_disabled("NP007") => {
                    if let Some(node) = query
                        .capture_index_for_name("np_apply_in_loop")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        let (line, col) = position(&node);
                        diags.push(
                            Diagnostic::new(
                                "NP007",
                                Severity::Warning,
                                path,
                                line,
                                col,
                                "`.apply(lambda ...)` inside a for loop — Python-level function applied element-by-element on every iteration",
                            )
                            .with_suggestion("Vectorise: apply the lambda to the whole column once outside the loop, or use `df.transform(fn)` / `df.assign(...)`")
                            .with_url("https://pandas.pydata.org/docs/user_guide/enhancingperf.html"),
                        );
                    }
                }

                _ => {}
            }
        }

        diags
    }
}

/// Does the inner subscript of `df["col"][row]` select by string key?
///
/// This is what distinguishes pandas column-then-row chained indexing (which
/// returns a copy, so assignments silently do not propagate) from ordinary
/// nested indexing on a list or ndarray.
fn inner_subscript_is_string_key(outer: tree_sitter::Node<'_>, _source: &[u8]) -> bool {
    let Some(inner) = outer.child_by_field_name("value") else {
        return false;
    };
    if inner.kind() != "subscript" {
        return false;
    }
    let Some(subscript) = inner.child_by_field_name("subscript") else {
        return false;
    };
    matches!(subscript.kind(), "string" | "concatenated_string")
}
