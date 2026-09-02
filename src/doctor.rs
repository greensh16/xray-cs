//! `xray doctor` — explain why xray did or did not fire on a file.
//!
//! Import gating is the architecture's load-bearing assumption and its most
//! opaque failure mode. A file whose only `import xarray` sits inside a
//! function body produces zero diagnostics and exits 0, which is
//! indistinguishable from a clean file. So does a file excluded by a
//! `[paths].exclude` glob, or one whose `xray.toml` came from a directory the
//! user forgot about.
//!
//! This command makes all of that visible rather than silent.

use crate::{cli::Cli, config::Config, ignore::IgnorePatterns, parser, rules};
use anyhow::Result;
use std::path::Path;

const BULLET: &str = "  •";

pub fn doctor(path: &str, cli: &Cli, config: &Config) -> Result<()> {
    println!("xray doctor — {path}\n");

    // ── File reachability ────────────────────────────────────────────────────
    let p = Path::new(path);
    if !p.exists() {
        println!("FILE\n{BULLET} does not exist");
        return Ok(());
    }
    let lintable = crate::runner::is_lintable_path(path);
    println!("FILE");
    println!(
        "{BULLET} extension is {}",
        if lintable {
            "lintable"
        } else {
            "NOT lintable — xray only reads .py and .ipynb"
        }
    );

    // ── Why the file might be skipped ────────────────────────────────────────
    let mut exclusions: Vec<String> = Vec::new();
    for pat in &config.paths.exclude {
        if glob::Pattern::new(pat).is_ok_and(|g| g.matches_path(p)) {
            exclusions.push(format!("[paths].exclude glob `{pat}`"));
        }
    }
    if IgnorePatterns::load(".").is_ignored(path) {
        exclusions.push(".xrayignore".to_string());
    }
    if exclusions.is_empty() {
        println!("{BULLET} not excluded by config or .xrayignore");
    } else {
        for e in &exclusions {
            println!("{BULLET} EXCLUDED by {e}");
        }
    }

    // ── Config provenance ────────────────────────────────────────────────────
    println!("\nCONFIG");
    match &cli.config {
        Some(c) => println!("{BULLET} loaded from --config {}", c.display()),
        None => match Config::find_config_file(".") {
            Some(found) => println!("{BULLET} found xray.toml at {}", found.display()),
            None => println!("{BULLET} no xray.toml found — using defaults"),
        },
    }
    if !config.disable.is_empty() {
        let mut d: Vec<&str> = config.disable.iter().map(String::as_str).collect();
        d.sort_unstable();
        println!("{BULLET} disabled rules: {}", d.join(", "));
    }
    if !cli.disable.is_empty() {
        println!(
            "{BULLET} disabled via --disable: {}",
            cli.disable.join(", ")
        );
    }
    match config.min_severity {
        Some(m) => println!("{BULLET} min_severity = {m:?}"),
        None => println!("{BULLET} min_severity unset — every severity reported"),
    }

    // ── Imports and the domains they gate ────────────────────────────────────
    if !lintable {
        return Ok(());
    }
    let parsed = match parser::parse_file(path) {
        Ok(p) => p,
        Err(e) => {
            println!("\nPARSE\n{BULLET} failed: {e}");
            return Ok(());
        }
    };
    let imports = &parsed.imports;

    println!("\nIMPORTS (top-level only)");
    let flags: [(&str, bool); 7] = [
        ("xarray", imports.xarray),
        ("dask", imports.dask),
        ("numpy", imports.numpy),
        ("pandas", imports.pandas),
        ("netCDF4", imports.netcdf4),
        ("zarr", imports.zarr),
        ("h5py", imports.h5py),
    ];
    let detected: Vec<&str> = flags
        .iter()
        .filter(|(_, on)| *on)
        .map(|(n, _)| *n)
        .collect();
    if detected.is_empty() {
        println!("{BULLET} none detected");
    } else {
        println!("{BULLET} detected: {}", detected.join(", "));
    }
    if !imports.aliases.is_empty() {
        let mut a: Vec<String> = imports
            .aliases
            .iter()
            .map(|(b, m)| format!("{b} → {m}"))
            .collect();
        a.sort();
        println!("{BULLET} aliases: {}", a.join(", "));
    }

    println!("\nRULE DOMAINS");
    for (domain, gated) in [
        ("xarray  (XR001–XR011)", imports.xarray),
        ("dask    (DK001–DK009)", imports.dask),
        ("numpy   (NP001–NP007)", imports.numpy || imports.pandas),
        (
            "io      (IO001–IO006)",
            imports.netcdf4 || imports.zarr || imports.numpy || imports.h5py || imports.xarray,
        ),
    ] {
        println!(
            "{BULLET} {domain}  {}",
            if gated {
                "RUNS"
            } else {
                "skipped (not imported)"
            }
        );
    }

    if detected.is_empty() {
        println!(
            "\n{BULLET} No rules can fire. xray reads only TOP-LEVEL imports —\n\
             \x20   an `import xarray` inside a function body is invisible to the gate,\n\
             \x20   which is the most common reason a file reports nothing."
        );
    }

    // ── Suppressions ─────────────────────────────────────────────────────────
    let sup = &parsed.suppressions;
    if !sup.file_level.is_empty() || !sup.line_level.is_empty() {
        println!("\nSUPPRESSIONS");
        if !sup.file_level.is_empty() {
            let mut f: Vec<&str> = sup.file_level.iter().map(String::as_str).collect();
            f.sort_unstable();
            println!("{BULLET} file-level: {}", f.join(", "));
        }
        let mut lines: Vec<(&usize, &std::collections::HashSet<String>)> =
            sup.line_level.iter().collect();
        lines.sort_by_key(|(l, _)| **l);
        for (line, ids) in lines {
            let mut v: Vec<&str> = ids.iter().map(String::as_str).collect();
            v.sort_unstable();
            println!("{BULLET} line {line}: {}", v.join(", "));
        }
    }

    // ── What actually fires ──────────────────────────────────────────────────
    let diags = rules::run_all(&parsed, path, config);
    println!("\nRESULT");
    if diags.is_empty() {
        println!("{BULLET} 0 diagnostics");
    } else {
        let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
        for d in &diags {
            *counts.entry(d.rule_id).or_default() += 1;
        }
        let summary: Vec<String> = counts.iter().map(|(id, n)| format!("{id}×{n}")).collect();
        println!(
            "{BULLET} {} diagnostics: {}",
            diags.len(),
            summary.join(", ")
        );
        let fixable = diags.iter().filter(|d| d.fix.is_some()).count();
        if fixable > 0 {
            println!("{BULLET} {fixable} auto-fixable — run `xray fix {path}`");
        }
    }
    Ok(())
}
