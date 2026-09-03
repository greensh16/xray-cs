# xray

[![CI](https://github.com/greensh16/xray-cs/actions/workflows/ci.yml/badge.svg)](https://github.com/greensh16/xray-cs/actions/workflows/ci.yml)
[![release](https://img.shields.io/github/v/release/greensh16/xray-cs?sort=semver)](https://github.com/greensh16/xray-cs/releases/latest)
[![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)
[![docs](https://img.shields.io/badge/docs-wiki-blue)](https://github.com/greensh16/xray-cs/wiki)

<!-- Add these two once `xray-cs` is published to crates.io — until then they
     render "crates.io: not found":
[![crates.io](https://img.shields.io/crates/v/xray-cs.svg)](https://crates.io/crates/xray-cs)
[![downloads](https://img.shields.io/crates/d/xray-cs.svg)](https://crates.io/crates/xray-cs)
-->


A fast, self-contained Rust linter for scientific Python workflows on HPC systems.
Targets **xarray**, **dask**, **NumPy**, **pandas**, **SciPy** and **scientific I/O**
patterns that general-purpose linters (ruff, pylint) don't cover — and reads your
**HPC submission script** alongside the Python it launches.

**Zero Python runtime required** — ships as a single binary. Runs on Gadi, Setonix,
or any HPC cluster without loading a Python module.

📖 **[Full documentation is on the wiki.](https://github.com/greensh16/xray-cs/wiki)**

---

## Installation

```bash
cargo install xray-cs
```

Or download a pre-built binary from the [releases page](https://github.com/greensh16/xray-cs/releases/latest) —
no Rust toolchain needed, which is usually what you want on a login node:

```bash
curl -L https://github.com/greensh16/xray-cs/releases/download/v1.2.0/xray-linux-x86_64 \
  -o ~/.local/bin/xray && chmod +x ~/.local/bin/xray
```

> **On the name.** The crate is published as **`xray-cs`** ("xray, climate science")
> because `xray` was already taken on crates.io. Only the registry name differs — the
> command is `xray`, the config is `xray.toml`, suppressions stay `# xray: disable=`.

---

## Usage

```bash
xray                          # lint every .py and .ipynb in the project
xray src/analysis.py          # a single file
xray --job run.sh analysis.py # also cross-check the Slurm/PBS resource request
xray fix src/                 # apply the mechanical fixes, printing a diff
xray --diff HEAD~1            # only files changed since the last commit
xray --watch                  # re-lint on save
xray explain XR012            # rationale and a bad/good example for one rule
xray doctor analysis.py       # why did (or didn't) xray fire on this file?
xray init                     # write an annotated xray.toml
```

Exit codes: `0` nothing at or above `--fail-on` · `1` findings at or above it ·
`2` fatal error. `--fail-on` defaults to `error`, so warnings and hints are reported
without failing the build.

Output formats: `text` (default), `json`, `sarif` (GitHub Code Scanning),
`gitlab-codequality`.

---

## Rules

**47 rules** across six library domains plus the HPC job-script domain, and one
cross-domain check. All IDs are stable.

| Domain | IDs | Covers |
|--------|-----|--------|
| **XR** | XR001–XR012 | Eager loads, `.values`, loops over dimensions, pathological chunk sizes |
| **DK** | DK001–DK010 | `.compute()` in loops, `.persist()` misuse, unchunked arrays, bad rechunks |
| **NP** | NP001–NP007 | Vectorisation anti-patterns, missing `dtype=`, deprecated APIs |
| **PD** | PD001–PD005 | Nested `iterrows()`, removed `.append()`, chained assignment, CSV I/O |
| **SP** | SP001–SP002 | `quad()` in loops, explicit matrix inversion |
| **IO** | IO001–IO006 | Uncompressed stores, direct netCDF4 access, per-iteration reads |
| **JOB** | JOB001–JOB005 | Allocation vs. what the Python actually does — **opt-in**, needs `--job` |
| — | XR000 | A suppression comment that suppressed nothing |

Seven rules carry a mechanical rewrite (`xray fix`): XR001, XR008, XR009, DK007,
NP004, NP006, NP007.

→ **[Rule reference](https://github.com/greensh16/xray-cs/wiki/Rule-Reference)** —
every rule with rationale, examples and severities. `xray rules` prints the same
table for your installed version.

---

## Configuration

`xray.toml` is discovered by walking up from the project root. Unknown keys are
rejected rather than ignored, so a typo fails loudly:

```toml
disable = ["IO001"]
min_severity = "hint"

[severity_overrides]
XR001 = "error"

[paths]
exclude = ["tests/fixtures/**"]
```

Suppress inline with `# xray: disable=XR001` or `# xray: disable-file=XR001`.
Run `xray init` for an annotated template covering every section.

→ **[Configuration guide](https://github.com/greensh16/xray-cs/wiki/Configuration)** ·
**[Suppressions](https://github.com/greensh16/xray-cs/wiki/Suppressions)**

---

## Documentation

| Page | |
|------|---|
| [Rule reference](https://github.com/greensh16/xray-cs/wiki/Rule-Reference) | All 47 rules, rationale and examples |
| [HPC job script rules](https://github.com/greensh16/xray-cs/wiki/Job-Rules) | `--job` — checking `#SBATCH` / `#PBS` against your Python |
| [Configuration](https://github.com/greensh16/xray-cs/wiki/Configuration) | Full `xray.toml` schema, env vars, CLI reference |
| [Suppressions](https://github.com/greensh16/xray-cs/wiki/Suppressions) | Silencing a rule for a line, file, path or project |
| [JSON output schema](https://github.com/greensh16/xray-cs/wiki/JSON-Output-Schema) | Stable v1 field reference |
| [HPC deployment cookbook](https://github.com/greensh16/xray-cs/wiki/HPC-Deployment-Cookbook) | Gadi, Setonix, PBS, Slurm, CI |
| [Case studies](https://github.com/greensh16/xray-cs/wiki/Case-Studies) | Real regressions xray caught |

---

## Scope

xray is **purely syntactic** — it reads source text without executing it or resolving
types. Rules fire on API names, resolved through the file's import aliases, so a call
is only attributed to a library the file actually imports.

It cannot see through indirection: a handle passed between functions, a keyword
supplied via `**kwargs` (those rules stay quiet rather than guess), or anything needing
runtime shape or dtype information. It reads only **top-level** imports, so an
`import xarray` inside a function body disables the xarray rules for that file —
`xray doctor <file>` will tell you when that has happened.

For general Python quality, run **ruff** alongside xray.

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for the rule-authoring workflow, including
tree-sitter query authoring. To request a rule, use the
[rule request template](.github/ISSUE_TEMPLATE/rule-request.md).

## License

Licensed under either **MIT** or **Apache-2.0**, at your option.
