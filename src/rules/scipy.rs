//! SciPy rules (SP001–SP002).
//!
//! Gated on `import scipy` alone. Both rules resolve the callee back to scipy
//! through the import table rather than trusting the method name: `quad` and
//! `inv` are short, generic names that any project might bind to something
//! else entirely.

use std::sync::LazyLock;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Query, QueryCursor};

use crate::{
    config::Config,
    diagnostic::{Diagnostic, RuleMeta, Severity},
    parser::{ParsedFile, call_module, is_inside_loop, position},
};

use super::RuleSet;

pub struct ScipyRules;

const QUERY_SRC: &str = include_str!("../../queries/scipy.scm");

/// Compiled once per process and shared across all rayon workers.
/// A compilation failure is a bug in xray itself, so we fail loudly.
static QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&tree_sitter_python::LANGUAGE.into(), QUERY_SRC)
        .unwrap_or_else(|e| panic!("xray: BUG — failed to compile scipy query: {e}"))
});

impl RuleSet for ScipyRules {
    fn meta() -> Vec<RuleMeta> {
        vec![
            RuleMeta {
                id: "SP001",
                name: "quad-in-loop",
                severity: Severity::Warning,
                description: "scipy.integrate.quad() inside a loop — use quad_vec() for one adaptive pass over the whole vector",
            },
            RuleMeta {
                id: "SP002",
                name: "explicit-matrix-inverse",
                severity: Severity::Warning,
                description: "scipy.linalg.inv() — prefer solve(); an explicit inverse is slower and numerically inferior",
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
                // SP001 — quad() in a loop
                0 if !config.is_disabled("SP001") => {
                    if let Some(node) = query
                        .capture_index_for_name("sp_quad_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        if !is_inside_loop(node) {
                            continue;
                        }
                        if call_module(node, source, &file.imports) != Some("scipy") {
                            continue;
                        }
                        let (line, col) = position(&node);
                        diags.push(
                            Diagnostic::new(
                                "SP001",
                                Severity::Warning,
                                path,
                                line,
                                col,
                                "`quad()` inside a loop — QUADPACK's workspace, error control and subdivision are rebuilt from scratch on every iteration",
                            )
                            .with_suggestion(
                                "Use `scipy.integrate.quad_vec(f, a, b)`: one adaptive pass integrates the whole vector, sharing the subdivision across components",
                            )
                            .with_fix_hint("quad_vec(f, a, b)")
                            .with_url("https://docs.scipy.org/doc/scipy/reference/generated/scipy.integrate.quad_vec.html"),
                        );
                    }
                }

                // SP002 — explicit matrix inverse
                1 if !config.is_disabled("SP002") => {
                    if let Some(node) = query
                        .capture_index_for_name("sp_inv_call")
                        .and_then(|i| m.nodes_for_capture_index(i).next())
                    {
                        if call_module(node, source, &file.imports) != Some("scipy") {
                            continue;
                        }
                        let (line, col) = position(&node);
                        diags.push(
                            Diagnostic::new(
                                "SP002",
                                Severity::Warning,
                                path,
                                line,
                                col,
                                "`inv()` forms an explicit inverse — `solve()` is roughly 2× faster and loses far less precision on an ill-conditioned matrix",
                            )
                            .with_suggestion(
                                "Replace `inv(A) @ b` with `scipy.linalg.solve(A, b)`; if you need it many times over the same A, factor once with `lu_factor` / `cho_factor` and reuse it",
                            )
                            .with_fix_hint("scipy.linalg.solve(A, b)")
                            .with_url("https://docs.scipy.org/doc/scipy/reference/generated/scipy.linalg.solve.html"),
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

    fn ids(src: &str) -> Vec<&'static str> {
        let parsed = parse_source(src.to_string()).unwrap();
        let mut diags = ScipyRules::check(&parsed, "<test>", &Config::default());
        diags.sort_by_key(|d| (d.line, d.rule_id));
        diags.into_iter().map(|d| d.rule_id).collect()
    }

    fn fires(rule: &'static str, src: &str) -> bool {
        ids(src).contains(&rule)
    }

    #[test]
    fn sp001_fires_in_a_loop_only() {
        let imports = "import scipy.integrate as integrate\n";
        assert!(fires(
            "SP001",
            &format!("{imports}for k in ks:\n    v, e = integrate.quad(f, 0, k)\n")
        ));
        assert!(!fires(
            "SP001",
            &format!("{imports}v, e = integrate.quad(f, 0, 1)\n")
        ));
    }

    #[test]
    fn sp001_resolves_every_import_spelling() {
        // `from scipy import integrate` binds a *name*, not an alias — the
        // receiver resolved to nothing until `call_module` learned to fall
        // back to the from-import table.
        assert!(fires(
            "SP001",
            "from scipy import integrate\nfor k in ks:\n    integrate.quad(f, 0, k)\n"
        ));
        assert!(fires(
            "SP001",
            "from scipy.integrate import quad\nfor k in ks:\n    quad(f, 0, k)\n"
        ));
        assert!(fires(
            "SP001",
            "import scipy\nfor k in ks:\n    scipy.integrate.quad(f, 0, k)\n"
        ));
    }

    #[test]
    fn sp001_ignores_a_same_named_call_from_elsewhere() {
        assert!(!fires(
            "SP001",
            "import scipy\nfor k in ks:\n    mylib.quad(f, 0, k)\n"
        ));
    }

    #[test]
    fn sp002_flags_scipy_inv_anywhere() {
        assert!(fires(
            "SP002",
            "import scipy\nx = scipy.linalg.inv(A) @ b\n"
        ));
        assert!(fires(
            "SP002",
            "from scipy.linalg import inv\nx = inv(A) @ b\n"
        ));
        assert!(!fires(
            "SP002",
            "import scipy\nx = scipy.linalg.solve(A, b)\n"
        ));
        // numpy's inverse is not in this rule's remit — the ID is SP, and the
        // docs say so.
        assert!(!fires(
            "SP002",
            "import scipy\nimport numpy as np\nx = np.linalg.inv(A)\n"
        ));
    }
}
