//! Pandas rules (PD001–PD005).
//!
//! The NP domain already carries a handful of pandas rules for historical
//! reasons; these are the ones added in v1.2 and they live in their own
//! domain, gated on `import pandas` alone rather than on numpy-or-pandas.
//!
//! Two of them deliberately overlap an NP rule on a narrower, worse case
//! (PD001 with NP001, PD003 with NP005). `rules::REDUNDANT_WITH` collapses the
//! pair when both land on one position, so a nested `iterrows()` reports once,
//! as the more specific finding.

use std::sync::LazyLock;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Node, Query, QueryCursor};

use crate::{
    bindings::Origin,
    config::Config,
    diagnostic::{Diagnostic, RuleMeta, Severity},
    parser::{ParsedFile, call_module, is_inside_loop, keyword_arg_present_or_unknown, position},
};

use super::RuleSet;

pub struct PandasRules;

const QUERY_SRC: &str = include_str!("../../queries/pandas.scm");

/// Compiled once per process and shared across all rayon workers.
/// A compilation failure is a bug in xray itself, so we fail loudly.
static QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&tree_sitter_python::LANGUAGE.into(), QUERY_SRC)
        .unwrap_or_else(|e| panic!("xray: BUG — failed to compile pandas query: {e}"))
});

impl RuleSet for PandasRules {
    fn meta() -> Vec<RuleMeta> {
        vec![
            RuleMeta {
                id: "PD001",
                name: "iterrows-in-loop",
                severity: Severity::Error,
                description: "DataFrame.iterrows() inside an enclosing loop — row-by-row Python iteration repeated every outer iteration",
            },
            RuleMeta {
                id: "PD002",
                name: "dataframe-append",
                severity: Severity::Error,
                description: "DataFrame.append() was removed in pandas 2.0 — use pd.concat()",
            },
            RuleMeta {
                id: "PD003",
                name: "chained-assignment",
                severity: Severity::Error,
                description: "Chained assignment df[...][...] = ... — writes to a temporary copy; use .loc / .iloc",
            },
            RuleMeta {
                id: "PD004",
                name: "read-csv-without-dtype",
                severity: Severity::Hint,
                description: "pd.read_csv() without dtype= — forces a type-inference pass over the whole file",
            },
            RuleMeta {
                id: "PD005",
                name: "to-csv-with-index",
                severity: Severity::Hint,
                description: ".to_csv() without index=False — writes a spurious index column that reappears as `Unnamed: 0`",
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
                // PD001 — iterrows() inside an enclosing loop
                0 if !config.is_disabled("PD001") => {
                    if let Some(node) = query
                        .capture_index_for_name("pd_iterrows_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        // `for i, row in df.iterrows():` puts the call in the
                        // loop *header*, which runs once — `is_inside_loop`
                        // already excludes that, so what reaches here is a
                        // genuinely nested iteration.
                        if !is_inside_loop(node) {
                            continue;
                        }
                        let (line, col) = position(&node);
                        diags.push(
                            Diagnostic::new(
                                "PD001",
                                Severity::Error,
                                path,
                                line,
                                col,
                                "`.iterrows()` inside an enclosing loop — the whole row-by-row Python pass is repeated on every outer iteration",
                            )
                            .with_suggestion(
                                "Hoist the frame out of the loop and vectorise: merge the two levels into one `df.groupby(...)`, or index with `.loc` on a precomputed mask",
                            )
                            .with_url("https://github.com/greensh16/xray-cs/wiki/Pandas-Rules#pd001"),
                        );
                    }
                }

                // PD002 — DataFrame.append(), removed in pandas 2.0
                1 if !config.is_disabled("PD002") => {
                    if let (Some(call_node), Some(recv)) = (
                        query
                            .capture_index_for_name("pd_append_call")
                            .and_then(|i| m.nodes_for_capture_index(i).next()),
                        query
                            .capture_index_for_name("pd_append_recv")
                            .and_then(|i| m.nodes_for_capture_index(i).next()),
                    ) {
                        // A *known* pandas receiver is required — see the note
                        // in the query. `list.append` must never be flagged.
                        if file.bindings.origin_of(recv, source, &file.imports)
                            != Some(Origin::Pandas)
                        {
                            continue;
                        }
                        let (line, col) = position(&call_node);
                        diags.push(
                            Diagnostic::new(
                                "PD002",
                                Severity::Error,
                                path,
                                line,
                                col,
                                "`DataFrame.append()` was removed in pandas 2.0 — this raises AttributeError, it is not a deprecation warning",
                            )
                            .with_suggestion(
                                "Collect the frames in a list and call `pd.concat(frames, ignore_index=True)` once — appending in a loop was O(n²) even before removal",
                            )
                            .with_fix_hint("pd.concat([df, other], ignore_index=True)")
                            .with_url("https://pandas.pydata.org/docs/whatsnew/v2.0.0.html#removal-of-prior-version-deprecations-changes"),
                        );
                    }
                }

                // PD003 — chained assignment
                2 if !config.is_disabled("PD003") => {
                    if let (Some(assign), Some(inner)) = (
                        query
                            .capture_index_for_name("pd_chained_assign")
                            .and_then(|i| m.nodes_for_capture_index(i).next()),
                        query
                            .capture_index_for_name("pd_chained_inner")
                            .and_then(|i| m.nodes_for_capture_index(i).next()),
                    ) {
                        // `grid[0][1] = x` on a list of lists is ordinary
                        // nested indexing and works perfectly well. Require
                        // evidence this is a DataFrame: either a string column
                        // key, or a receiver known to be pandas.
                        if !looks_like_dataframe_chain(inner, source, file) {
                            continue;
                        }
                        let (line, col) = position(&assign);
                        diags.push(
                            Diagnostic::new(
                                "PD003",
                                Severity::Error,
                                path,
                                line,
                                col,
                                "Chained assignment — the first subscript may return a copy, so this write can be silently discarded (SettingWithCopyWarning; a no-op under copy-on-write in pandas 3.0)",
                            )
                            .with_suggestion(
                                "Use a single indexer: `df.loc[row_selection, \"col\"] = value`",
                            )
                            .with_url("https://pandas.pydata.org/docs/user_guide/indexing.html#returning-a-view-versus-a-copy"),
                        );
                    }
                }

                // PD004 — read_csv without dtype=
                3 if !config.is_disabled("PD004") => {
                    if let Some(call_node) = query
                        .capture_index_for_name("pd_read_csv_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        // dask and pyarrow have read_csv too, and their dtype
                        // story is different — only pandas' is this rule's.
                        if call_module(call_node, source, &file.imports) != Some("pandas") {
                            continue;
                        }
                        if keyword_arg_present_or_unknown(call_node, source, "dtype") {
                            continue;
                        }
                        let (line, col) = position(&call_node);
                        diags.push(
                            Diagnostic::new(
                                "PD004",
                                Severity::Hint,
                                path,
                                line,
                                col,
                                "`read_csv()` without `dtype=` infers every column's type by scanning the whole file, then usually settles on float64 or object",
                            )
                            .with_suggestion(
                                "Pass `dtype={\"id\": \"int32\", \"value\": \"float32\"}` — and `usecols=` if you do not need every column",
                            )
                            .with_url("https://pandas.pydata.org/docs/reference/api/pandas.read_csv.html"),
                        );
                    }
                }

                // PD005 — to_csv without index=False
                4 if !config.is_disabled("PD005") => {
                    if let Some(call_node) = query
                        .capture_index_for_name("pd_to_csv_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        if keyword_arg_present_or_unknown(call_node, source, "index") {
                            // An explicit `index=` is a decision, whichever way
                            // it went; this rule is about the default.
                            continue;
                        }
                        // `.to_csv()` on something that is not a frame — a
                        // custom writer, say — is not ours to comment on.
                        if !receiver_may_be_pandas(call_node, source, file) {
                            continue;
                        }
                        let (line, col) = position(&call_node);
                        diags.push(
                            Diagnostic::new(
                                "PD005",
                                Severity::Hint,
                                path,
                                line,
                                col,
                                "`.to_csv()` without `index=False` writes the index as a nameless leading column — it comes back as `Unnamed: 0` on the next read",
                            )
                            .with_suggestion(
                                "Pass `index=False`, or `index=True` explicitly if the index is real data you mean to keep",
                            )
                            .with_fix_hint("index=False")
                            .with_url("https://pandas.pydata.org/docs/reference/api/pandas.DataFrame.to_csv.html"),
                        );
                    }
                }

                _ => {}
            }
        }

        diags
    }
}

/// Is `inner` — the first subscript of a `a[..][..]` chain — plausibly a
/// DataFrame column selection rather than nested list/array indexing?
///
/// Either piece of evidence is enough: a string key (`df["col"][0]`, the
/// spelling NP005 keys on) or a receiver the binding tracker already knows to
/// be pandas (`df[mask]["col"]`).
fn looks_like_dataframe_chain(inner: Node<'_>, source: &[u8], file: &ParsedFile) -> bool {
    let string_key = inner
        .child_by_field_name("subscript")
        .is_some_and(|s| matches!(s.kind(), "string" | "concatenated_string"));
    if string_key {
        return true;
    }
    inner
        .child_by_field_name("value")
        .and_then(|v| file.bindings.origin_of(v, source, &file.imports))
        == Some(Origin::Pandas)
}

/// Could the receiver of this method call be a pandas object?
///
/// `None` — an unknown receiver, such as a function parameter — counts as
/// "maybe", which is the convention every receiver-tracking rule follows:
/// tracking may only ever remove false positives, never add silence.
fn receiver_may_be_pandas(call_node: Node<'_>, source: &[u8], file: &ParsedFile) -> bool {
    let Some(recv) = call_node
        .child_by_field_name("function")
        .and_then(|f| f.child_by_field_name("object"))
    else {
        return false;
    };
    !matches!(
        file.bindings.origin_of(recv, source, &file.imports),
        Some(Origin::Plain) | Some(Origin::Xarray)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_source;

    /// Rule IDs fired by `src`, in line order. Calls `PandasRules::check`
    /// directly, so `run_all`'s redundancy filter is deliberately not in the
    /// way.
    fn ids(src: &str) -> Vec<&'static str> {
        let parsed = parse_source(src.to_string()).unwrap();
        let mut diags = PandasRules::check(&parsed, "<test>", &Config::default());
        diags.sort_by_key(|d| (d.line, d.rule_id));
        diags.into_iter().map(|d| d.rule_id).collect()
    }

    fn fires(rule: &'static str, src: &str) -> bool {
        ids(src).contains(&rule)
    }

    const IMPORTS: &str = "import pandas as pd\n";

    #[test]
    fn pd001_needs_an_enclosing_loop_not_just_iterrows() {
        // The ordinary spelling: the call is in the loop header, evaluated
        // once. NP001 covers that; PD001 must not double-report it.
        assert!(!fires(
            "PD001",
            &format!("{IMPORTS}for i, row in df.iterrows():\n    pass\n")
        ));
        // Nested: the whole row-by-row pass repeats per outer iteration.
        assert!(fires(
            "PD001",
            &format!("{IMPORTS}for g in groups:\n    for i, row in df.iterrows():\n        pass\n")
        ));
    }

    #[test]
    fn pd002_requires_a_known_pandas_receiver() {
        assert!(fires(
            "PD002",
            &format!("{IMPORTS}df = pd.DataFrame(d)\nout = df.append(other)\n")
        ));
        // The reason this rule cannot use the usual unknown-is-maybe rule:
        // list.append is everywhere.
        assert!(!fires(
            "PD002",
            &format!("{IMPORTS}parts = []\nparts.append(x)\n")
        ));
        assert!(!fires(
            "PD002",
            &format!("{IMPORTS}def f(items):\n    items.append(1)\n")
        ));
    }

    #[test]
    fn pd003_flags_chained_assignment_not_nested_list_writes() {
        assert!(fires("PD003", &format!("{IMPORTS}df['a'][0] = 1\n")));
        assert!(fires(
            "PD003",
            &format!("{IMPORTS}df = pd.read_csv('a.csv', dtype='f4')\ndf[mask]['a'] = 1\n")
        ));
        // A list of lists: nested indexing assigns exactly where it says.
        assert!(!fires("PD003", &format!("{IMPORTS}grid[1][2] = 0\n")));
        // A read, not a write — that is NP005's.
        assert!(!fires("PD003", &format!("{IMPORTS}v = df['a'][0]\n")));
    }

    #[test]
    fn pd004_read_csv_without_dtype() {
        assert!(fires(
            "PD004",
            &format!("{IMPORTS}df = pd.read_csv('a.csv')\n")
        ));
        assert!(!fires(
            "PD004",
            &format!("{IMPORTS}df = pd.read_csv('a.csv', dtype={{'a': 'int32'}})\n")
        ));
        // dask's read_csv is not pandas'.
        assert!(!fires(
            "PD004",
            "import pandas as pd\nimport dask.dataframe as dd\ndf = dd.read_csv('a.csv')\n"
        ));
        // `**opts` may well carry dtype — a definite miss cannot be claimed.
        assert!(!fires(
            "PD004",
            &format!("{IMPORTS}df = pd.read_csv('a.csv', **opts)\n")
        ));
    }

    #[test]
    fn pd005_to_csv_without_an_explicit_index_decision() {
        assert!(fires("PD005", &format!("{IMPORTS}df.to_csv('out.csv')\n")));
        assert!(!fires(
            "PD005",
            &format!("{IMPORTS}df.to_csv('out.csv', index=False)\n")
        ));
        // An explicit index=True is a decision, not the default.
        assert!(!fires(
            "PD005",
            &format!("{IMPORTS}df.to_csv('out.csv', index=True)\n")
        ));
    }
}
