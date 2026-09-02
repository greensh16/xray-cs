//! File-watch mode for `xray --watch`.
//!
//! On startup, performs a full lint of all matched Python files.  Then it
//! watches the file system for changes and re-lints any modified `.py` file
//! as soon as the change is detected.
//!
//! Uses the `notify` crate which selects the best OS-level watcher available
//! (inotify on Linux, kqueue on macOS, FSEvents on macOS 10.7+,
//! ReadDirectoryChangesW on Windows) and falls back gracefully when those
//! mechanisms are unavailable (e.g. over NFS/Lustre on HPC nodes).
//!
//! Usage:
//!   xray --watch                # watch all Python files recursively
//!   xray --watch src/           # watch a specific directory
//!   xray --watch analysis.py    # watch a single file

use anyhow::Result;
use notify::{
    Config as NotifyConfig, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant},
};

use crate::{cli::Cli, config::Config, ignore::IgnorePatterns, runner};

/// Run the watch loop.  Lints all matching files on start, then re-lints on
/// every `.py` file-save event.  Blocks until the user presses Ctrl-C.
pub fn run_watch(cli: &Cli, config: &Config) -> Result<()> {
    // ── Initial lint ──────────────────────────────────────────────────────────
    eprintln!("xray: starting watch mode (Ctrl-C to stop)");
    eprintln!("{}", "─".repeat(72));
    lint_and_print_paths(cli, &collect_watch_paths(cli, config)?, config);
    eprintln!("{}", "─".repeat(72));

    // ── Set up file watcher ───────────────────────────────────────────────────
    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
    let mut watcher = RecommendedWatcher::new(tx, NotifyConfig::default())?;

    // Determine which paths to watch.  If the user supplied explicit file
    // paths, watch their parent directories; otherwise watch the roots.
    let watch_roots = watch_roots(cli);
    for root in &watch_roots {
        let mode = if Path::new(root).is_file() {
            RecursiveMode::NonRecursive
        } else {
            RecursiveMode::Recursive
        };
        if let Err(e) = watcher.watch(Path::new(root), mode) {
            eprintln!("xray: cannot watch {root}: {e}");
        }
    }

    let ignore = IgnorePatterns::load(".");

    // ── Event loop ─────────────────────────────────────────────────────────
    // Debounce: collect events for up to 50 ms so that editors that write
    // in multiple steps (write temp, rename) trigger only one lint cycle.
    let debounce = Duration::from_millis(50);
    let mut pending: HashSet<PathBuf> = HashSet::new();
    let mut last_event = Instant::now();

    loop {
        // Try to drain events with a timeout
        match rx.recv_timeout(debounce) {
            Ok(Ok(event)) => {
                if is_modify_event(&event) {
                    for path in event.paths {
                        if is_python_file(&path) && !ignore.is_ignored(path.to_str().unwrap_or(""))
                        {
                            pending.insert(path);
                            last_event = Instant::now();
                        }
                    }
                }
            }
            Ok(Err(e)) => eprintln!("xray: watch error: {e}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        // Flush pending paths once the debounce window has passed
        if !pending.is_empty() && last_event.elapsed() >= debounce {
            let paths: Vec<String> = pending
                .drain()
                .filter_map(|p| p.to_str().map(String::from))
                .collect();

            eprintln!();
            eprintln!(
                "xray: {} file{} changed — re-linting...",
                paths.len(),
                if paths.len() == 1 { "" } else { "s" }
            );
            eprintln!("{}", "─".repeat(72));
            lint_and_print_paths(cli, &paths, config);
            eprintln!("{}", "─".repeat(72));
        }
    }

    Ok(())
}

// ── internal helpers ──────────────────────────────────────────────────────────

fn watch_roots(cli: &Cli) -> Vec<String> {
    if cli.paths.is_empty() {
        vec![".".to_string()]
    } else {
        cli.paths.clone()
    }
}

fn collect_watch_paths(cli: &Cli, config: &Config) -> Result<Vec<String>> {
    let raw_patterns: Vec<String> = if cli.paths.is_empty() {
        config.paths.include.clone()
    } else {
        cli.paths.clone()
    };

    // Same expansion as a batch run: files, directories and globs all work.
    let mut paths = runner::collect_paths_pub(&raw_patterns)?;

    // Apply excludes and .xrayignore
    let ignore = IgnorePatterns::load(".");
    paths.retain(|p| {
        !config.paths.exclude.iter().any(|ex| {
            glob::Pattern::new(ex)
                .map(|pat| pat.matches_path(Path::new(p)))
                .unwrap_or(false)
        }) && !ignore.is_ignored(p)
    });

    Ok(paths)
}

/// Re-lint `paths` through the normal runner pipeline so that `--min-severity`,
/// `--disable`, `[severity_overrides]` and `--format` behave exactly as they do
/// in a batch run.
fn lint_and_print_paths(cli: &Cli, paths: &[String], config: &Config) {
    if paths.is_empty() {
        eprintln!("  no files to lint");
        return;
    }
    // Feed the resolved paths back in as positional arguments; everything else
    // (formatting, filtering, notebook handling) is the runner's job.
    let mut sub = cli.clone();
    sub.paths = paths.to_vec();
    sub.watch = false;
    sub.diff = None;
    sub.stats = false;
    if let Err(e) = runner::run(&sub, config) {
        eprintln!("xray: {e}");
    }
}

/// Watch every file type xray can lint, notebooks included — batch mode
/// already handles `.ipynb`, so watch mode should too.
fn is_python_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| runner::LINTABLE_EXTENSIONS.contains(&e))
}

fn is_modify_event(event: &Event) -> bool {
    // `Remove` is deliberately absent: a deleted file cannot be linted, and
    // treating deletions as changes produced a "could not parse" error for
    // every file the user removed.
    matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_))
}
