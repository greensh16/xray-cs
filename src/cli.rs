use clap::{Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use serde::Deserialize;
use std::path::PathBuf;

/// HPC scientific Python linter — xarray, dask, NumPy, IO.
///
/// Exit codes:
///   0  — no diagnostics at or above --fail-on
///   1  — one or more diagnostics at or above --fail-on (default: error)
///   2  — internal error (parse failure, I/O error, bug)
#[derive(Parser, Debug, Clone)]
#[command(name = "xray", version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<XrayCommand>,

    // ── Lint options (used when no subcommand is given) ──────────────────────
    /// Python files or glob patterns to analyse (default: **/*.py)
    #[arg(num_args = 0..)]
    pub paths: Vec<String>,

    /// Path to xray.toml config file  [env: XRAY_CONFIG]
    #[arg(long, short = 'c', env = "XRAY_CONFIG")]
    pub config: Option<PathBuf>,

    /// Output format  [env: XRAY_FORMAT]
    #[arg(long, short = 'f', default_value = "text", env = "XRAY_FORMAT")]
    pub format: OutputFormat,

    /// Minimum severity to report  [env: XRAY_MIN_SEVERITY]
    ///
    /// Takes precedence over `min_severity` in xray.toml.  When neither is
    /// set, every severity is reported.
    #[arg(long, short = 's', env = "XRAY_MIN_SEVERITY")]
    pub min_severity: Option<MinSeverity>,

    /// List all available rules and exit
    #[arg(long)]
    pub list_rules: bool,

    /// Disable specific rules (comma-separated, e.g. --disable XR001,NP004)  [env: XRAY_DISABLE]
    #[arg(long, value_delimiter = ',', env = "XRAY_DISABLE")]
    pub disable: Vec<String>,

    /// Print a per-rule and per-file summary table after linting
    #[arg(long)]
    pub stats: bool,

    /// Lowest severity that makes xray exit non-zero  [env: XRAY_FAIL_ON]
    ///
    /// Diagnostics are still reported regardless; this only controls the exit
    /// code.  `never` always exits 0.  Defaults to `error`, so a file with
    /// only warnings exits 0 unless you ask otherwise.
    #[arg(long, default_value = "error", env = "XRAY_FAIL_ON")]
    pub fail_on: FailOn,

    /// Only lint Python files changed relative to a git ref
    ///
    /// Runs `git diff --name-only --diff-filter=ACMR <REF>` and lints only
    /// the resulting .py files.  Useful for PR checks without re-linting the
    /// entire codebase.
    ///
    /// Examples:
    ///   xray --diff HEAD~1
    ///   xray --diff origin/main
    #[arg(long, value_name = "REF")]
    pub diff: Option<String>,

    /// Apply available auto-fixes instead of only reporting them
    ///
    /// Equivalent to `xray fix <paths>`.  Prints a diff of every change.
    #[arg(long)]
    pub fix: bool,

    /// With --fix or `xray fix`, show the diff without writing anything
    #[arg(long)]
    pub dry_run: bool,

    /// Watch for file changes and re-lint automatically
    ///
    /// Performs an initial lint of all matching files, then watches for saves
    /// and re-lints each changed file as it is modified.
    ///
    /// Examples:
    ///   xray --watch
    ///   xray --watch src/
    #[arg(long)]
    pub watch: bool,
}

#[derive(Subcommand, Debug, Clone)]
pub enum XrayCommand {
    /// Show detailed rationale, bad/good examples, and docs for a rule
    Explain {
        /// Rule ID to explain (e.g. XR001 or np004 — case-insensitive)
        rule_id: String,
    },

    /// Scaffold an annotated xray.toml in the current directory
    Init {
        /// Overwrite an existing xray.toml
        #[arg(long)]
        force: bool,
    },

    /// Start the Language Server Protocol server (stdin/stdout JSON-RPC)
    ///
    /// Compatible with any LSP client: VS Code (via the xray extension),
    /// Neovim (nvim-lspconfig), Emacs (lsp-mode / eglot), and others.
    ///
    /// The server lints files on open and save, publishing diagnostics
    /// back to the editor in real time.
    Lsp,

    /// Rewrite files in place, applying every available auto-fix
    ///
    /// Prints a unified diff of each change.  Only rules with a verifiable
    /// mechanical rewrite are fixed; the rest stay advisory.  Notebooks are
    /// never rewritten.
    ///
    /// Examples:
    ///   xray fix src/
    ///   xray fix --dry-run src/
    Fix {
        /// Files or globs to fix (default: the same set `xray` would lint)
        #[arg(num_args = 0..)]
        paths: Vec<String>,

        /// Show the diff without writing anything
        #[arg(long)]
        dry_run: bool,
    },

    /// List rules as a table or as machine-readable JSON
    ///
    /// The JSON form is the source of truth for generated documentation:
    /// every rule's id, name, domain, severity, description, docs URL and
    /// auto-fix eligibility.
    ///
    /// Examples:
    ///   xray rules
    ///   xray rules --format json
    Rules {
        /// Output format
        #[arg(long, short = 'f', default_value = "text")]
        format: RuleListFormat,
    },

    /// Explain why xray did or did not fire on a file
    ///
    /// Prints the resolved import context and which rule domains it gates,
    /// the config that was selected and where it came from, and whether the
    /// path is excluded.  Use this when a file reports nothing unexpectedly.
    ///
    /// Examples:
    ///   xray doctor analysis.py
    Doctor {
        /// File to diagnose
        path: String,
    },

    /// Print shell completion script to stdout
    ///
    /// Usage examples:
    ///   xray completions bash >> ~/.bash_completion
    ///   xray completions zsh  > ~/.zfunc/_xray
    ///   xray completions fish > ~/.config/fish/completions/xray.fish
    Completions {
        /// Target shell
        shell: Shell,
    },
}

/// Output format for `xray rules`.
#[derive(ValueEnum, Clone, Debug, PartialEq)]
pub enum RuleListFormat {
    /// Aligned table, as `--list-rules` prints
    Text,
    /// JSON array of rule metadata objects
    Json,
}

/// Output format for diagnostics.
#[derive(ValueEnum, Clone, Debug, PartialEq)]
pub enum OutputFormat {
    /// Human-readable text with source context (default)
    Text,
    /// JSON array of diagnostic objects
    Json,
    /// SARIF 2.1.0 — for GitHub Code Scanning and other SARIF-aware platforms
    Sarif,
    /// GitLab Code Quality report JSON — for GitLab CI artifact upload
    #[value(name = "gitlab-codequality")]
    GitlabCodequality,
}

/// Lowest severity that should make the process exit non-zero.
#[derive(ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FailOn {
    Hint,
    Warning,
    #[default]
    Error,
    /// Never fail, whatever is found.
    Never,
}

#[derive(ValueEnum, Clone, Copy, Debug, Default, PartialEq, PartialOrd, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MinSeverity {
    #[default]
    Hint,
    Warning,
    Error,
}

pub fn parse() -> Cli {
    Cli::parse()
}
