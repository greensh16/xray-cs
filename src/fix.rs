//! Mechanical source rewrites for auto-fix-eligible rules.
//!
//! A [`Fix`] is a single byte-range replacement computed by the rule that
//! emitted the diagnostic. Fixes are deliberately narrow: every one currently
//! shipped is an intra-line edit (insert a keyword argument, rename an
//! attribute), because a rewrite that spans statements cannot be verified
//! syntactically and a linter that silently produces invalid Python is worse
//! than one that only advises.
//!
//! Rules whose "fix" would be a structural rewrite — NP002's
//! accumulate-in-a-list transformation, for instance — stay suggestion-only.
//! See `fix_eligible` in `src/explain.rs`.

use crate::diagnostic::Diagnostic;
use serde::Serialize;

/// Rules `xray fix` can actually apply.
///
/// The single source of truth: `xray rules --format json` reports
/// `fix_eligible` from this list, and a test pins every `ExplainEntry` to it,
/// so a rule cannot advertise a fix it does not implement.
///
/// Deliberately absent:
/// - **NP002** — the accumulate-in-a-list rewrite spans statements and cannot
///   be verified syntactically; a wrong rewrite here silently changes results.
/// - **XR006** — `to_array(dim=…)` needs a dimension *name*, which is a
///   modelling decision, not a mechanical one.
/// - **IO006** — swapping `engine="scipy"` for `"netcdf4"` requires a package
///   the environment may not have.
pub const FIXABLE_RULES: &[&str] = &[
    "XR001", "XR008", "XR009", "DK007", "NP004", "NP006", "NP007",
];

/// Can `xray fix` rewrite this rule's findings?
pub fn is_fixable(rule_id: &str) -> bool {
    FIXABLE_RULES.contains(&rule_id)
}

/// A single mechanical edit: replace `[start_byte, end_byte)` with `replacement`.
///
/// Byte offsets are into the **CRLF-normalised** source that the parser saw,
/// which is what [`apply`] expects. Line/column are carried alongside for SARIF,
/// whose `replacements` are expressed as regions rather than offsets.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Fix {
    /// Short human description, e.g. `add chunks="auto"`.
    pub description: String,
    pub replacement: String,
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: usize,
    pub start_column: usize,
    pub end_line: usize,
    pub end_column: usize,
}

impl Fix {
    /// Build a fix from a byte range, deriving line/column from `source`.
    pub fn new(
        source: &str,
        start_byte: usize,
        end_byte: usize,
        replacement: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        let (start_line, start_column) = offset_to_line_col(source, start_byte);
        let (end_line, end_column) = offset_to_line_col(source, end_byte);
        Self {
            description: description.into(),
            replacement: replacement.into(),
            start_byte,
            end_byte,
            start_line,
            start_column,
            end_line,
            end_column,
        }
    }

    /// An insertion at `at`, changing nothing else.
    pub fn insert(
        source: &str,
        at: usize,
        text: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self::new(source, at, at, text, description)
    }
}

/// 1-based (line, column) for a byte offset.
fn offset_to_line_col(source: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(source.len());
    let before = &source[..offset];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, col)
}

/// Outcome of fixing one file.
#[derive(Debug, Default)]
pub struct FixOutcome {
    pub applied: usize,
    /// Fixes skipped because they overlapped an already-applied edit.
    pub skipped_overlapping: usize,
    pub original: String,
    pub fixed: String,
}

impl FixOutcome {
    pub fn changed(&self) -> bool {
        self.original != self.fixed
    }
}

/// Apply every fix attached to `diags` to `source`.
///
/// Edits are applied last-to-first so earlier offsets stay valid. Overlapping
/// fixes are dropped rather than merged — two rules rewriting the same span
/// cannot both be right, and applying either silently would be a guess.
pub fn apply(source: &str, diags: &[Diagnostic]) -> FixOutcome {
    let mut fixes: Vec<&Fix> = diags.iter().filter_map(|d| d.fix.as_ref()).collect();
    // Later edits first; for equal starts, the wider edit first so the
    // narrower one is the overlap that gets dropped.
    fixes.sort_by(|a, b| {
        b.start_byte
            .cmp(&a.start_byte)
            .then(b.end_byte.cmp(&a.end_byte))
    });

    let mut out = source.to_string();
    let mut applied = 0usize;
    let mut skipped = 0usize;
    // Lowest byte touched so far; a fix must end at or before this.
    let mut floor = usize::MAX;

    for fix in fixes {
        if fix.end_byte > floor || fix.start_byte > fix.end_byte || fix.end_byte > out.len() {
            skipped += 1;
            continue;
        }
        if !out.is_char_boundary(fix.start_byte) || !out.is_char_boundary(fix.end_byte) {
            skipped += 1;
            continue;
        }
        out.replace_range(fix.start_byte..fix.end_byte, &fix.replacement);
        floor = fix.start_byte;
        applied += 1;
    }

    FixOutcome {
        applied,
        skipped_overlapping: skipped,
        original: source.to_string(),
        fixed: out,
    }
}

/// Restore the original file's line endings.
///
/// The parser normalises CRLF→LF, and fix offsets are into that normalised
/// text — so writing the result straight back would silently convert a
/// Windows-authored file to LF and produce a diff touching every line.
pub fn restore_line_endings(original_raw: &str, fixed: &str) -> String {
    if original_raw.contains("\r\n") {
        fixed.replace('\n', "\r\n")
    } else {
        fixed.to_string()
    }
}

/// Render a minimal unified-style diff of the lines a fix touched.
///
/// Deliberately not a general diff: every shipped fix is an intra-line edit, so
/// the changed lines can be identified exactly rather than inferred. If a fix
/// ever changes the line count, this says so instead of printing something
/// misleading.
pub fn render_diff(path: &str, outcome: &FixOutcome) -> String {
    let before: Vec<&str> = outcome.original.lines().collect();
    let after: Vec<&str> = outcome.fixed.lines().collect();

    let mut out = format!("--- {path}\n+++ {path}\n");

    if before.len() != after.len() {
        out.push_str(&format!(
            "@@ file rewritten: {} lines → {} lines @@\n",
            before.len(),
            after.len()
        ));
        return out;
    }

    for (i, (b, a)) in before.iter().zip(after.iter()).enumerate() {
        if b != a {
            out.push_str(&format!("@@ line {} @@\n", i + 1));
            out.push_str(&format!("-{b}\n"));
            out.push_str(&format!("+{a}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{Diagnostic, Severity};

    fn diag_with(source: &str, start: usize, end: usize, rep: &str) -> Diagnostic {
        Diagnostic::new("XR001", Severity::Warning, "f.py", 1, 1, "m")
            .with_fix(Fix::new(source, start, end, rep, "test"))
    }

    #[test]
    fn applies_a_single_insertion() {
        let src = "f(a)";
        let d = vec![diag_with(src, 3, 3, ", b")];
        let o = apply(src, &d);
        assert_eq!(o.fixed, "f(a, b)");
        assert_eq!(o.applied, 1);
    }

    #[test]
    fn applies_multiple_fixes_back_to_front() {
        let src = "f(a)\ng(c)";
        let d = vec![diag_with(src, 3, 3, ", b"), diag_with(src, 8, 8, ", d")];
        let o = apply(src, &d);
        assert_eq!(o.fixed, "f(a, b)\ng(c, d)");
        assert_eq!(o.applied, 2);
    }

    #[test]
    fn drops_overlapping_fixes_rather_than_guessing() {
        let src = "math.sqrt(x)";
        let d = vec![
            diag_with(src, 0, 9, "np.sqrt"),
            diag_with(src, 5, 9, "cbrt"), // overlaps the first
        ];
        let o = apply(src, &d);
        assert_eq!(o.applied, 1);
        assert_eq!(o.skipped_overlapping, 1);
    }

    #[test]
    fn replacement_can_shorten_the_source() {
        let src = "np.matrix(x)";
        let d = vec![diag_with(src, 3, 9, "array")];
        assert_eq!(apply(src, &d).fixed, "np.array(x)");
    }

    #[test]
    fn no_fixes_leaves_the_source_untouched() {
        let src = "x = 1\n";
        let d = vec![Diagnostic::new(
            "XR001",
            Severity::Warning,
            "f.py",
            1,
            1,
            "m",
        )];
        let o = apply(src, &d);
        assert!(!o.changed());
        assert_eq!(o.applied, 0);
    }

    #[test]
    fn crlf_files_keep_their_line_endings() {
        let raw = "a\r\nb\r\n";
        let fixed_lf = "a\r\nb\r\n".replace("\r\n", "\n").replace('a', "z");
        let restored = restore_line_endings(raw, &fixed_lf);
        assert_eq!(restored, "z\r\nb\r\n");
    }

    #[test]
    fn lf_files_are_not_given_carriage_returns() {
        assert_eq!(restore_line_endings("a\nb\n", "z\nb\n"), "z\nb\n");
    }

    #[test]
    fn diff_shows_only_changed_lines() {
        let o = FixOutcome {
            applied: 1,
            skipped_overlapping: 0,
            original: "one\ntwo\nthree\n".into(),
            fixed: "one\nTWO\nthree\n".into(),
        };
        let d = render_diff("f.py", &o);
        assert!(d.contains("@@ line 2 @@"));
        assert!(d.contains("-two"));
        assert!(d.contains("+TWO"));
        assert!(!d.contains("one\n+"));
    }

    #[test]
    fn advertised_fixable_rules_match_the_explain_metadata() {
        // `fix_eligible` in `xray rules --format json` and `xray explain` must
        // describe the same set the fix engine implements — a rule that
        // advertises a fix nobody wrote is a promise the tool breaks.
        for meta in crate::rules::all_meta() {
            let advertised = crate::explain::entry_for(meta.id)
                .map(|e| e.fix_eligible)
                .unwrap_or(false);
            assert_eq!(
                advertised,
                is_fixable(meta.id),
                "{} advertises fix_eligible={advertised} but is_fixable={}",
                meta.id,
                is_fixable(meta.id)
            );
        }
    }

    #[test]
    fn line_col_is_one_based() {
        assert_eq!(offset_to_line_col("ab\ncd", 0), (1, 1));
        assert_eq!(offset_to_line_col("ab\ncd", 3), (2, 1));
        assert_eq!(offset_to_line_col("ab\ncd", 4), (2, 2));
    }
}
