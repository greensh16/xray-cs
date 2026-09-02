# xray

A fast, self-contained Rust linter for scientific Python workflows on HPC systems.
Targets **xarray**, **dask**, **NumPy**, **pandas**, and **scientific I/O** patterns
that general-purpose linters (ruff, pylint) don't cover.

**Zero Python runtime required** — ships as a single binary. Runs on Gadi, Setonix,
or any HPC cluster without loading a Python module.

---

## Installation

Download a pre-built binary from the [releases page](https://github.com/greensh16/xray/releases/latest):

```bash
curl -L https://github.com/greensh16/xray/releases/download/v1.1.0/xray-linux-x86_64 \
  -o ~/.local/bin/xray && chmod +x ~/.local/bin/xray
```

---

## Usage

```bash
xray                          # lint all .py files in the project
xray src/analysis.py          # single file
xray src/                     # a directory — every .py and .ipynb beneath it
xray analysis.ipynb           # Jupyter notebook — each code cell linted independently
xray --min-severity warning   # warnings and errors only
xray --format json src/ > report.json
xray --diff HEAD~1            # only files changed since last commit
xray --watch                  # re-lint on save
xray explain XR001            # show rationale and fix examples for a rule
xray init                     # write an annotated xray.toml to the current directory
```

### Fixing

```bash
xray fix src/                 # apply every available auto-fix, printing a diff
xray fix --dry-run src/       # show the diff without writing anything
xray src/ --fix               # same as `xray fix src/`
```

Seven rules carry a mechanical rewrite: XR001, XR008, XR009, DK007, NP004,
NP006, NP007. `xray rules --format json` reports `fix_eligible` per rule.

Fixes are deliberately narrow — each is an intra-line edit whose result is
verified to parse. Rules whose "fix" would restructure code (NP002's
accumulate-in-a-list rewrite) or require a judgement call (XR006's dimension
name) stay advisory. Original line endings and quote style are preserved, and
notebooks are never rewritten.

### Diagnosing

```bash
xray doctor analysis.py       # why did (or didn't) xray fire on this file?
xray rules                    # the rule table
xray rules --format json      # machine-readable rule metadata
```

`xray doctor` prints the resolved import context and which rule domains it
gates, which `xray.toml` is in effect and where it came from, any matching
exclusion, and what actually fired. Reach for it when a file reports nothing
unexpectedly — xray reads only **top-level** imports, so an `import xarray`
inside a function body silently disables every xarray rule for that file.

Exit codes: `0` nothing at or above `--fail-on` · `1` findings at or above it · `2` fatal error.

`--fail-on` defaults to `error`, so warnings and hints are reported without failing
the process. Use `--fail-on warning` to gate CI more tightly, or `--fail-on never`
to report without ever failing.

---

## Rules

34 rules across four domains plus one cross-domain check. All IDs are stable.
The **Fix** column marks rules `xray fix` can rewrite mechanically.
`xray --list-rules` prints this table; `xray explain <ID>` gives the rationale,
a bad/good example pair, and links to upstream docs.

### XR — xarray

| ID | Default | Fix | Description |
|----|---------|-----|-------------|
| XR001 | warning | ✓ | open_dataset/open_mfdataset called without chunks= — data loads eagerly into memory |
| XR002 | warning |  | .values accessed on a DataArray — materialises the full array and drops coordinates |
| XR003 | hint |  | for-loop iterating over a Dataset/DataArray attribute — prefer vectorised operations |
| XR004 | warning |  | .sel() called with a float literal — use method='nearest' or tolerance= to avoid silent misses |
| XR005 | error |  | .compute() called inside a for loop — triggers the full dask graph on every iteration |
| XR006 | warning |  | .to_array()/.to_dataarray() called without dim= — creates an unnamed 'variable' concat dimension |
| XR007 | error |  | xr.concat called inside a for loop — O(n²) intermediate copies; collect then concat once |
| XR008 | warning | ✓ | open_mfdataset called without parallel=True — files are opened serially |
| XR009 | warning | ✓ | apply_ufunc with dask='allowed' silently falls back to serial execution; use dask='parallelized' |
| XR010 | warning |  | xr.merge called inside a for loop — O(n²) cost; collect datasets then merge once |
| XR011 | hint |  | to_netcdf() called without encoding= — data written as float64 with no compression |

### DK — Dask

| ID | Default | Fix | Description |
|----|---------|-----|-------------|
| DK001 | error |  | .compute() called inside a for loop — rebuilds the full task graph every iteration |
| DK002 | error |  | dask.compute() called inside a for loop |
| DK003 | warning |  | More .compute() calls in one file than dask.compute_call_threshold — consider .persist() for reused graphs |
| DK004 | hint |  | Dask object constructed and immediately .compute()d — the graph never did any work, use pandas/numpy directly |
| DK005 | warning |  | .persist() result not assigned — cost of materialising the graph is paid with no benefit |
| DK006 | warning |  | .persist().compute() chain — persist() is redundant; just call .compute() directly |
| DK007 | warning | ✓ | da.from_array() called without chunks= — creates a single-chunk array that defeats dask parallelism |
| DK008 | warning |  | .rechunk() called inside a for loop — triggers a full graph materialisation on every iteration |
| DK009 | error |  | da.concatenate() inside a for loop — O(n²) intermediate copies; collect arrays then concatenate once |

### NP — NumPy / pandas

| ID | Default | Fix | Description |
|----|---------|-----|-------------|
| NP001 | warning |  | DataFrame.iterrows() — row-by-row Python iteration, use vectorised operations |
| NP002 | error |  | pd.concat / np.concatenate inside a loop — quadratic copy overhead |
| NP003 | hint |  | np.zeros/ones/empty called without dtype= — silently defaults to float64 |
| NP004 | warning | ✓ | math.* scalar function — replace with numpy ufunc; Warning in loops, Hint elsewhere |
| NP005 | warning |  | Chained indexing df[col][row] — creates a copy; assignments silently fail |
| NP006 | warning | ✓ | np.matrix() is deprecated since NumPy 1.16 — use np.array() / np.ndarray instead |
| NP007 | warning | ✓ | DataFrame.applymap() is deprecated (use .map()), or .apply(lambda) inside a loop |

### IO — Scientific I/O

| ID | Default | Fix | Description |
|----|---------|-----|-------------|
| IO001 | hint |  | np.save() used — uncompressed, unchunked; prefer Zarr or HDF5 for large arrays |
| IO002 | hint |  | netCDF4.Dataset opened directly — bypasses xarray coordinate alignment machinery |
| IO003 | warning |  | zarr.open called without chunks= — unchunked Zarr defeats compression and parallel I/O |
| IO004 | warning |  | netCDF4 variable subscripted inside a loop — each read may hit disk; pre-load outside the loop |
| IO005 | hint |  | h5py.File opened without swmr=True — consider SWMR mode for concurrent HPC read workflows |
| IO006 | warning |  | xr.open_dataset called with engine='scipy' — loads eagerly without chunking; use 'netcdf4' or 'zarr' |

### Cross-domain

| ID | Default | Fix | Description |
|----|---------|-----|-------------|
| XR000 | hint |  | A `# xray: disable=` comment that suppressed nothing — the rule no longer fires here |

---

## Configuration

`xray.toml` is discovered by walking up from the project root:

```toml
disable = ["IO001"]        # rule IDs are case-insensitive
min_severity = "hint"      # overridden by --min-severity / XRAY_MIN_SEVERITY

[severity_overrides]
XR001 = "error"   # promote to error
NP003 = "hint"    # demote to hint

[paths]
include = ["src/**/*.py", "notebooks/**/*.ipynb"]
exclude = ["tests/fixtures/**"]
```

Unknown keys are rejected rather than ignored, so a typo like `disabel = [...]`
fails loudly instead of silently doing nothing. Run `xray init` for an annotated
template covering every section.

Per-line suppression: `# xray: disable=XR001`
Per-file suppression: `# xray: disable-file=XR001`

Environment variables: `XRAY_CONFIG`, `XRAY_FORMAT`, `XRAY_MIN_SEVERITY`, `XRAY_DISABLE`, `XRAY_FAIL_ON`.

---

## Output Formats

| Flag | Use case |
|------|----------|
| `--format text` | Human-readable terminal output (default) |
| `--format json` | Versioned JSON envelope — see [JSON schema docs](https://github.com/greensh16/xray/wiki/JSON-Output-Schema) |
| `--format sarif` | GitHub Code Scanning / any SARIF 2.1.0 consumer |
| `--format gitlab-codequality` | GitLab CI Code Quality report |

---

## Documentation

Full documentation lives on the [GitHub Wiki](https://github.com/greensh16/xray/wiki):

- [Rule reference](https://github.com/greensh16/xray/wiki/Rule-Reference) — rationale, examples, and fix hints for all 34 rules
- [Configuration guide](https://github.com/greensh16/xray/wiki/Configuration) — full `xray.toml` schema
- [JSON output schema](https://github.com/greensh16/xray/wiki/JSON-Output-Schema) — stable v1 field reference
- [HPC deployment cookbook](https://github.com/greensh16/xray/wiki/HPC-Deployment-Cookbook) — Gadi, Setonix, PBS, Slurm
- [Case studies](https://github.com/greensh16/xray/wiki/Case-Studies) — real-world performance regressions caught by xray

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for a step-by-step guide to proposing and
implementing new rules, including the tree-sitter query authoring workflow.

To request a new rule, use the [rule request issue template](.github/ISSUE_TEMPLATE/rule-request.md).

## Scope

xray uses syntactic analysis — it reads source text without executing it or
resolving types. Rules fire on API names, resolved through the file's import
aliases (`import xarray as xr`, `import dask.array as dsa`), so a call is only
attributed to a library the file actually imports.

It cannot see through indirection: a handle passed between functions, a keyword
supplied via `**kwargs` (those rules stay quiet rather than guess), or anything
needing runtime shape/dtype information. For general Python quality, run **ruff**
alongside xray.
