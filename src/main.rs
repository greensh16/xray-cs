use clap::CommandFactory;
use clap_complete::generate;
use std::io;
use xray::{cli, config::Config, doctor, explain, init, lsp, runner, watch};

/// Load the config a command should run under: `--config` if given, else the
/// nearest `xray.toml` walking up from the working directory.
fn load_config(cli: &cli::Cli) -> Config {
    let result = match &cli.config {
        Some(path) => Config::from_file(path),
        None => Config::from_dir("."),
    };
    result.unwrap_or_else(|e| {
        eprintln!("xray: could not load config: {e}");
        std::process::exit(2);
    })
}

/// Apply `--jobs` / `XRAY_JOBS` to rayon's global pool.
///
/// Must run before the first `par_iter`, since the pool is built once on first
/// use and cannot be reconfigured afterwards. A failure here is not fatal: the
/// lint is still correct with the default pool, so it warns and continues.
fn configure_thread_pool(jobs: Option<usize>) {
    let Some(n) = jobs else { return };
    if n == 0 {
        eprintln!("xray: --jobs must be at least 1; using the default thread count");
        return;
    }
    if let Err(e) = rayon::ThreadPoolBuilder::new()
        .num_threads(n)
        .build_global()
    {
        eprintln!("xray: could not set thread count to {n}: {e}");
    }
}

fn main() {
    let cli = cli::parse();
    configure_thread_pool(cli.jobs);

    match &cli.command {
        // ── xray explain <RULE_ID> ────────────────────────────────────────────
        Some(cli::XrayCommand::Explain { rule_id }) => {
            // `explain` reports the unknown-rule error itself; printing a
            // second message here meant one bad ID produced two different
            // lines of output.
            if !explain::explain(rule_id) {
                std::process::exit(2);
            }
        }

        // ── xray init [--force] ───────────────────────────────────────────────
        Some(cli::XrayCommand::Init { force }) => {
            if let Err(e) = init::init(*force) {
                eprintln!("{e}");
                std::process::exit(2);
            }
        }

        // ── xray fix [paths] [--dry-run] ──────────────────────────────────────
        Some(cli::XrayCommand::Fix { paths, dry_run }) => {
            let config = load_config(&cli);
            let dry = *dry_run || cli.dry_run;
            if let Err(e) = runner::run_fix(&cli, &config, paths, dry) {
                eprintln!("xray: fatal error: {e}");
                std::process::exit(2);
            }
        }

        // ── xray rules [--format json] ────────────────────────────────────────
        Some(cli::XrayCommand::Rules { format }) => match format {
            cli::RuleListFormat::Json => {
                if let Err(e) = runner::print_rule_list_json() {
                    eprintln!("xray: {e}");
                    std::process::exit(2);
                }
            }
            cli::RuleListFormat::Text => runner::print_rule_list(),
        },

        // ── xray doctor <file> ────────────────────────────────────────────────
        Some(cli::XrayCommand::Doctor { path }) => {
            let config = load_config(&cli);
            if let Err(e) = doctor::doctor(path, &cli, &config) {
                eprintln!("xray: {e}");
                std::process::exit(2);
            }
        }

        // ── xray clean ────────────────────────────────────────────────────────
        Some(cli::XrayCommand::Clean) => {
            let path = std::path::Path::new(xray::cache::CACHE_FILE);
            match std::fs::remove_file(path) {
                Ok(()) => println!("xray: removed {}", path.display()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    println!("xray: no {} to remove", path.display());
                }
                Err(e) => {
                    eprintln!("xray: could not remove {}: {e}", path.display());
                    std::process::exit(2);
                }
            }
        }

        // ── xray completions <shell> ──────────────────────────────────────────
        Some(cli::XrayCommand::Completions { shell }) => {
            let mut cmd = cli::Cli::command();
            generate(*shell, &mut cmd, "xray", &mut io::stdout());
        }

        // ── xray lsp ──────────────────────────────────────────────────────────
        Some(cli::XrayCommand::Lsp) => {
            lsp::run_lsp();
        }

        // ── Default: lint files (or watch, or fix) ────────────────────────────
        None => {
            let config = load_config(&cli);

            // `xray --fix <paths>` is the inline spelling of `xray fix <paths>`.
            if cli.fix {
                if let Err(e) = runner::run_fix(&cli, &config, &cli.paths, cli.dry_run) {
                    eprintln!("xray: fatal error: {e}");
                    std::process::exit(2);
                }
                std::process::exit(0);
            }

            if cli.watch {
                if let Err(e) = watch::run_watch(&cli, &config) {
                    eprintln!("xray: watch error: {e}");
                    std::process::exit(2);
                }
                std::process::exit(0);
            }

            let results = runner::run(&cli, &config).unwrap_or_else(|e| {
                eprintln!("xray: fatal error: {e}");
                std::process::exit(2);
            });

            let exit_code = if results.should_fail(cli.fail_on) {
                1
            } else {
                0
            };
            std::process::exit(exit_code);
        }
    }
}
