use clap::CommandFactory;
use clap_complete::generate;
use std::io;
use xray::{cli, config::Config, explain, init, lsp, runner, watch};

fn main() {
    let cli = cli::parse();

    match &cli.command {
        // ── xray explain <RULE_ID> ────────────────────────────────────────────
        Some(cli::XrayCommand::Explain { rule_id }) => {
            if !explain::explain(rule_id) {
                eprintln!("xray: unknown rule `{rule_id}`. Run `xray --list-rules` for valid IDs.");
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

        // ── xray completions <shell> ──────────────────────────────────────────
        Some(cli::XrayCommand::Completions { shell }) => {
            let mut cmd = cli::Cli::command();
            generate(*shell, &mut cmd, "xray", &mut io::stdout());
        }

        // ── xray lsp ──────────────────────────────────────────────────────────
        Some(cli::XrayCommand::Lsp) => {
            lsp::run_lsp();
        }

        // ── Default: lint files (or watch) ────────────────────────────────────
        None => {
            let config = match &cli.config {
                Some(path) => Config::from_file(path).unwrap_or_else(|e| {
                    eprintln!("xray: could not load config: {e}");
                    std::process::exit(2);
                }),
                None => Config::from_dir(".").unwrap_or_else(|e| {
                    eprintln!("xray: could not load config: {e}");
                    std::process::exit(2);
                }),
            };

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
