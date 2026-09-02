//! Git diff integration for `xray --diff <REF>`.
//!
//! Calls `git diff --name-only --relative --diff-filter=ACMR <REF>` and returns
//! the subset of changed files xray can lint (`.py`, `.ipynb`).
//!
//! `--relative` matters: without it git reports paths from the repository root,
//! so running `xray --diff origin/main` from a subdirectory produced a "could
//! not parse" error for every changed file.
//!
//! `--diff-filter=ACMR` selects Added, Copied, Modified, and Renamed files,
//! deliberately excluding Deleted files which no longer exist on disk.

use anyhow::{Context, Result};

/// Return the list of `.py` files that differ between the working tree and
/// `git_ref` (e.g. `HEAD~1`, `origin/main`, a commit SHA).
///
/// Returns an error if `git` is not found, the repository check fails, or
/// the ref is invalid.
pub fn changed_python_files(git_ref: &str) -> Result<Vec<String>> {
    let output = std::process::Command::new("git")
        .args([
            "diff",
            "--name-only",
            "--relative",
            "--diff-filter=ACMR",
            git_ref,
        ])
        .output()
        .with_context(
            || "failed to run `git diff` — is git installed and is this a git repository?",
        )?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!(
            "`git diff --name-only {git_ref}` failed: {stderr}"
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_diff_output(&stdout))
}

/// Parse the raw multi-line stdout of `git diff --name-only` into a filtered
/// list of `.py` paths.  Used for testing without spawning a subprocess.
pub fn parse_diff_output(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && crate::runner::is_lintable_path(l))
        .map(str::to_string)
        .collect()
}

// ── unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_non_python_files() {
        let raw = "src/analysis.py\nREADME.md\nsetup.cfg\nsrc/utils.py\n";
        let result = parse_diff_output(raw);
        assert_eq!(result, vec!["src/analysis.py", "src/utils.py"]);
    }

    #[test]
    fn empty_diff_produces_empty_list() {
        let result = parse_diff_output("");
        assert!(result.is_empty());
    }

    #[test]
    fn only_non_python_files_produces_empty_list() {
        let raw = "Makefile\ndocs/index.rst\npyproject.toml\n";
        let result = parse_diff_output(raw);
        assert!(result.is_empty());
    }

    #[test]
    fn handles_blank_lines() {
        let raw = "\nsrc/model.py\n\nsrc/io.py\n\n";
        let result = parse_diff_output(raw);
        assert_eq!(result, vec!["src/model.py", "src/io.py"]);
    }
}
