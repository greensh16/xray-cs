# Changelog

All notable changes to xray are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
xray uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

---

## [1.1.0] — 2026-09-02

The v1.1 milestone: auto-fix and developer experience, plus the accuracy and
hardening work that closed the post-1.0 review.

No rule IDs, config keys or output schemas changed incompatibly. One behavioural
change is worth reading before upgrading: **DK004 no longer flags
`ds.mean().compute()`** — see *Changed* below.

### Added — v1.1 milestone

- **`xray fix`** — applies mechanical rewrites in place, printing a unified diff
  of every change. `xray <paths> --fix` is the inline spelling; `--dry-run`
  shows the diff without writing. Seven rules carry a fix: **XR001** and
  **DK007** (insert `chunks="auto"`), **XR008** (`parallel=True`), **XR009**
  (`dask="parallelized"`), **NP004** (`math.*` → the file's numpy binding),
  **NP006** (`matrix()` → `array()`) and **NP007** (`applymap()` → `map()`).

  Fixes are deliberately narrow. Each is an intra-line edit; the result is
  asserted to parse, fixes are idempotent, overlapping edits are dropped rather
  than merged, original line endings and quote style are preserved, and
  notebooks are never rewritten. Rules whose rewrite would restructure code
  (**NP002**) or require a judgement call (**XR006**'s dimension name,
  **IO006**'s engine swap needing a package that may not be installed) stay
  advisory — `src/fix.rs` records why for each.

- **Fix annotations in JSON and SARIF.** Diagnostics gain a `fix` object, and
  SARIF results now carry real `artifactChanges`/`replacements` that
  SARIF-aware tooling can apply. Previously every SARIF fix had an empty
  `artifactChanges` array, which is well-formed but actionable by nothing.

- **`xray doctor <file>`** — explains why xray did or did not fire on a file:
  resolved import context and the domains it gates, which `xray.toml` is in
  effect and where it came from, matching exclusions, suppressions, and what
  actually fired. Aimed at the most opaque failure mode in the architecture — a
  file whose only `import xarray` sits inside a function body reports nothing
  and exits 0, indistinguishable from a clean file.

- **`xray rules [--format json]`** — machine-readable rule metadata (id, name,
  domain, severity, description, docs URL, `fix_eligible`). This is now the
  source of truth for generated documentation; the README rule tables are
  generated from it rather than hand-maintained, which is what produced the
  drift found in the v1.0 review.

- **`extends` in `xray.toml`** — inherit a shared config, resolved relative to
  the declaring file. Local keys override inherited ones; `disable`,
  `severity_overrides` and `per_file_ignores` merge rather than replace, so
  extending a profile never silently drops its rules. Cycles are reported with
  the full chain. Remote URLs are deliberately unsupported — see
  `docs/configuration.md` for why.

- **`[per_file_ignores]` in `xray.toml`** — glob-scoped rule disables, with
  `"*"` to skip every rule for a path. Previously the only lever was the global
  `disable` list, so one noisy rule in one directory cost coverage everywhere.

- **XR000 — stale suppression detection.** Reports a `# xray: disable=` comment
  that suppressed nothing. Only line-level suppressions are checked;
  `disable-file=` legitimately guards a file that is currently clean. It found
  two real stale suppressions in xray's own `clean.py` fixture on first run,
  left behind by this release's DK004 change.

### Changed

- **Receiver tracking (`src/bindings.rs`).** xray now records what each name was
  assigned from within its enclosing function — `ds = xr.open_dataset(...)`,
  `df = pd.read_csv(...)` — so rules can check a receiver's library instead of
  inferring it from a method name. Scopes reset at every function boundary,
  parameters stay unknown, and a name rebound from a conflicting origin degrades
  to unknown rather than guessing. There is no interprocedural analysis.

  Rules treat "unknown" as *keep the previous behaviour*, so this only removes
  false positives; it never trades them for false negatives. Costs about 13%
  more CPU per file (0.55 s → 0.62 s over 500 files) with no change in wall time.

- **DK004 no longer flags reduce-then-compute.** `ds.mean().compute()` and
  `ds.sel(...).compute()` are the correct dask idiom — the graph ran a parallel
  reduction and `.compute()` retrieves the small result. The rule now fires only
  when a dask object is constructed or loaded and materialised in the same
  expression (`da.from_array(x).compute()`, `dd.read_csv(f).compute()`), which is
  what its own documentation and `xray explain DK004` always described.

- **NP004 no longer hints on genuine scalars.** Outside a loop the rule fires
  only when its argument is known to be an array. On a single float `math.sqrt`
  is *faster* than the NumPy ufunc, which pays array-dispatch overhead for one
  value, so the hint previously pointed the wrong way. Inside a loop the warning
  is unchanged — there the iteration itself is the problem.

- **`fix_eligible` no longer over-promises.** XR006 and IO006 advertised a fix
  that was never implemented. `src/fix.rs` is now the single source of truth and
  a test pins every `ExplainEntry` to it, so a rule cannot claim a fix nobody
  wrote.

- **NP003 describes `np.full` correctly.** `np.full(shape, 0)` infers `int64`
  from the fill value; it does not default to `float64` like `zeros`, `ones` and
  `empty`. The message now says so.

### Packaging

- **Release binaries are stripped.** `[profile.release] strip = true` drops debug
  symbols from published binaries — roughly 15% smaller to download onto a login
  node, with no practical loss: a release backtrace was not useful anyway and
  panics still report their message. `[profile.bench]` opts back out, since it
  inherits from `release` and stripping would take symbol names out of
  profiler output.

- **Linux release binaries are now statically linked against musl.**
  `release.yml` builds `x86_64-unknown-linux-musl` and
  `aarch64-unknown-linux-musl` via `cross`, so the published binaries run on HPC
  login nodes and older distros regardless of host glibc — a gnu-linked binary
  built on ubuntu-22.04 requires glibc 2.35+, excluding RHEL/CentOS 7-8 and
  SLES 12. Artifact names are unchanged, so the GitHub Action and the documented
  `curl` install commands keep working.

### Tests

- **Unit tests in every rule module**, which `CONTRIBUTING.md` has always required
  and none of them had: 44 tests covering all 33 rule IDs, each pairing a
  fires-case with the nearest legitimate lookalike that must stay silent, plus
  coverage of the four config knobs rules actually read. They call
  `RuleSet::check` directly rather than `rules::run_all`, so a failure points at
  one rule set instead of a shared fixture.
- `CONTRIBUTING.md`'s unit-test example called a per-rule `check_xr008(&parsed)`
  helper that has never existed; it now shows the real API.

### Fixed

- **XR002 no longer fires on non-xarray receivers.** `df.values` on a DataFrame
  is the documented pandas idiom, and `.values` on a NumPy array or plain
  container has nothing to do with xarray. A receiver whose origin cannot be
  determined — a function parameter, say — is still flagged.

- **NP007b missed `.apply(lambda ...)` whose result was assigned.** The rule
  expressed loop context in the query via `(_)* … (_)*`, which only matched
  direct `expression_statement` children of the loop body, so
  `df[col] = df[col].apply(lambda x: ...)` — the exact example in the rule's own
  documentation — was never reported. That construct could also match at several
  split points and emit duplicate diagnostics for one call. Loop context now uses
  `is_inside_loop`, as every other loop-sensitive rule does.

- **Suppression comments inside strings and docstrings no longer suppress
  anything.** Suppressions are now collected from the AST's `comment` nodes
  instead of by scanning raw lines for `# xray:`, so a docstring demonstrating
  how to silence a rule stopped silencing it for real — a `disable-file=` in a
  docstring previously took out the whole file.

- **Inline suppression rule IDs are case-insensitive.** `# xray: disable=xr001`
  now works, matching `--disable` and `xray.toml`, which have accepted lowercase
  since 1.0.1.

- **CRLF files no longer render misplaced diagnostic markers.** `render_text`
  normalises line endings exactly as the parser does; previously every ariadne
  label drifted by one character per preceding line.

- **`CONTRIBUTING.md`'s architecture and rule-authoring sections were fiction.**
  They described inline query strings, per-rule `check_<id>()` functions, a
  `parsed.path` field, a `.with_fix()` builder and an `ExplainEntry` shape — none
  of which exist. A contributor following them would have failed at every step.
  Rewritten against the real codebase.

- **`xray explain <unknown-id>` printed two different error messages** for one
  error: `explain()` reported it and `main` reported it again in different words.
  `main` now just exits 2.

- **`docs/rules/dask.md` documented a rule DK004 does not implement** (it
  described `dask.compute()` with a single argument), and `docs/rules/numpy.md`
  described NP003 as flagging `np.array` rather than the allocators it actually
  checks. Both sections rewritten.

---

## [1.0.1] — 2026-09-02

A correctness and accuracy release. Every change is a fix to behaviour that
shipped in 1.0.0; no rule IDs, config keys or output schemas changed
incompatibly.

### Added

- **`--fail-on <hint|warning|error|never>`** (env `XRAY_FAIL_ON`) — controls the
  lowest severity that makes xray exit non-zero, independently of what is
  reported. Defaults to `error`, which is exactly 1.0.0's behaviour, so existing
  pipelines are unaffected. `never` always exits 0, for report-only runs.
- **`min_severity` in `xray.toml`** — the key was documented but silently
  ignored; it is now read, and `--min-severity` on the command line overrides it.
- **`cell` field on notebook diagnostics** in JSON output, alongside the real
  notebook path in `file`.
- **SARIF `originalUriBaseIds` / `uriBaseId`** and `properties.notebookCell`.
- **Two regression fixtures** — `tests/fixtures/false_positives.py`, which must
  stay silent, and `tests/fixtures/loop_context.py`, which pins comprehension,
  `while`-loop and loop-header behaviour.

### Changed

- **Directory arguments are walked recursively.** `xray src/` previously matched
  nothing and exited 0 — a silent pass. Paths are now expanded, normalised and
  deduplicated, so the same file passed twice is linted once.
- **Rule IDs are case-insensitive** in both `--disable` and `xray.toml`, matching
  the validation that already accepted them.
- **Unknown `xray.toml` keys are now rejected** rather than silently ignored, so
  a typo'd key fails loudly instead of quietly doing nothing.
- **Text output goes to stdout** rather than stderr.
- **`--diff` uses `--relative --diff-filter=ACMR`** and includes `.ipynb` files.
- **Watch mode honours every filter** — severity, disables, format and config are
  applied to re-lints exactly as in a batch run.
- **The published crate ships its fixtures and benches**, so `cargo test` works
  from a registry checkout.

### Performance

- **Tree-sitter queries are compiled once per process** instead of once per file.
  Linting 500 files went from 23.19 s to 0.55 s of CPU (3.51 s to 0.12 s wall) —
  roughly 45× less CPU.
- **Source files are read once per file** when rendering text output, rather than
  once per diagnostic.

### Fixed

Rule accuracy — each of these fired on correct code:

- **XR003** flagged any attribute iterated in a `for` loop (`for f in self.files`);
  it is now gated on an allow-list of dimension names.
- **XR007 / XR010 / DK007 / DK009 / IO001 / IO003 / IO005** identified their target
  by receiver name alone, so `pd.concat` was reported as `xr.concat` and the
  builtin `open()` as `zarr.open()`. Receivers are now resolved through the file's
  import aliases, which also makes `import dask.array as dsa` work.
- **IO004** flagged every subscript inside a loop, including plain list and dict
  indexing; it now requires a netCDF4-derived handle.
- **NP005** flagged any double subscript, including `grid[i][j]`; it now requires a
  string key.
- **IO006** could never fire, because the I/O domain was not gated on `xarray`.
- **`**kwargs` spread no longer defeats the "missing keyword" rules** — XR001,
  XR006, XR011, DK007 and IO005 stay silent when a call forwards `**kwargs`,
  where the argument may well be present.
- **XR004** now matches negative float coordinates (`ds.sel(lat=-33.5)`).
- **Loop detection covers `while` loops and comprehensions**, and no longer fires
  on calls in the loop header itself.
- **One `.compute()` in a loop produces one diagnostic**, not the two or three
  that XR005, DK001 and DK002 previously emitted at the same position.
- **DK003** fires when the count exceeds `compute_call_threshold`, matching how
  the threshold is documented, and reports the first call past it.

Correctness elsewhere:

- **`--stats` attributed issues to the wrong files** when any file produced no
  diagnostics, because results were zipped against the input list positionally.
- **The LSP server dropped `[severity_overrides]`**, `disable` and `min_severity`,
  so editor diagnostics disagreed with the CLI.
- **LSP percent-decoding was Latin-1**, so a path containing `%C3%A9` resolved to
  `Ã©` and the file was not found. `Content-Length` is now matched
  case-insensitively.
- **Watch mode** treated file deletion as a modification and printed a parse
  error, and ignored `.ipynb` files that batch mode linted.
- **GitLab Code Quality fingerprints** could collide between two diagnostics on
  the same line.
- **XR003's suggestion printed a literal `{dim}`** instead of the dimension name.
- **The GitHub Action downloaded a release asset that is never published**, and
  its `issues-found` output was a boolean rather than a count.
- **The pre-commit hook definition** pinned a stale revision and documented an
  install command that did not work.

---

## [1.0.0] — 2026-03-19

### Added

- **Jupyter notebook support** — `.ipynb` files are now linted directly without
  any conversion step. Diagnostics include the cell number and per-cell
  line/column (e.g. `analysis.ipynb:cell[3]:2:5`). IPython magic lines
  (`%`/`!`) are stripped before parsing so they don't cause syntax errors, while
  preserving per-cell line numbers. Import context is accumulated across all
  cells so `import xarray` in cell 1 correctly gates xarray rules in cell 5.
- **XR008** — `open_mfdataset` without `parallel=True` (Warning): flags calls
  that open multi-file datasets without concurrent file-open via `dask.delayed`.
- **XR009** — `apply_ufunc` with `dask="allowed"` (Warning): flags the silent
  serial fallback mode; recommends `dask="parallelized"`.
- **XR010** — `xr.merge` inside a `for` loop (Warning): O(n²) coordinate
  alignment; collect datasets first then merge once.
- **XR011** — `to_netcdf()` without `encoding=` (Hint): variables written as
  float64 with no compression; suggests dtype + zlib encoding.
- **DK007** — `da.from_array()` without `chunks=` (Warning): single monolithic
  chunk defeats all Dask parallelism.
- **DK008** — `.rechunk()` inside a `for` loop (Warning): O(n) full
  re-partitions on an ever-growing array.
- **DK009** — `da.concatenate()` inside a `for` loop (Error): same O(n²)
  anti-pattern as XR007 / NP002 but for Dask arrays.
- Integration tests and `ExplainEntry` entries for all 7 new rules.
- Rule count updated to 32 across all docs and README.

---

## [0.9.0] — 2026-03-17

### Added

- **Stable JSON output schema** — `--format json` now emits a versioned envelope
  object with `schema_version: "1"`, a `diagnostics` array, and a `summary` object
  (`total`, `errors`, `warnings`, `hints`). Documented in `docs/json-schema.md`.
  The `build_json()` function is now public for consumers of the Rust library.
- **CRLF line-ending normalisation** — `parse_file()` and `parse_source()` now
  normalise `\r\n` to `\n` before parsing, so diagnostic line numbers are
  correct on files created on Windows or checked out with `core.autocrlf=true`.
- **Non-UTF-8 source hardening** — `parse_file()` reads bytes with
  `String::from_utf8_lossy` rather than `read_to_string`, replacing invalid
  bytes with the replacement character instead of returning `Err`. Non-ASCII
  paths are now handled correctly on all platforms.
- **Cross-platform CI matrix** — `.github/workflows/ci.yml` builds and tests
  on Linux x86-64, Linux aarch64 (via `cross`), macOS arm64, and Windows x86-64
  on every push and pull request. Release workflow creates GitHub releases and
  publishes to crates.io.
- **crates.io publish metadata** — `Cargo.toml` now includes `license`,
  `repository`, `homepage`, `documentation`, `keywords`, `categories`, `readme`,
  and `exclude` fields so `cargo install xray` works after release.
- **`collect_paths_pub()`** — public wrapper around the internal glob helper,
  exposed for integration testing and advanced library consumers.

### Changed

- Stable rule IDs declared: all rule IDs from XR001–XR007, DK001–DK006,
  NP001–NP007, and IO001–IO006 are frozen. No renumbering before v2.0.
- Stable config schema declared: `xray.toml` keys are frozen; additions only,
  no removals until v2.0.
- VS Code extension bumped to 0.9.0.

### Tests Added

- CRLF source parses without error and produces correct line/column numbers.
- Unicode multi-byte characters in source (CJK, accented, combining) do not
  shift line numbers or cause panics.
- Non-UTF-8 bytes (Latin-1) in source produce diagnostics via lossy conversion.
- `Config::from_file` returns `Err` for malformed TOML and missing files.
- Valid TOML config round-trips all fields correctly.
- CLI-level disable overrides config-level enables.
- Zero-match glob patterns return an empty vec without error.
- Literal file paths (non-glob) are collected as-is.
- Deeply nested `tests/**/*.py` glob matches all fixtures.
- JSON schema version field, diagnostics array, and summary counts are verified.

---

## [0.8.0] — 2026-03-17

### Added

- **Hosted documentation site** — full rule reference under `docs/rules/`, configuration
  guide at `docs/configuration.md`, HPC deployment cookbook at `docs/hpc-cookbook.md`,
  and per-rule "why this pattern is slow" explainers.
- **`CONTRIBUTING.md`** — step-by-step walkthrough for proposing, implementing, and
  testing a new rule, including the tree-sitter query authoring workflow.
- **Rule request issue template** — `.github/ISSUE_TEMPLATE/rule-request.md` with
  triage criteria and a structured proposal format for community-submitted rules.
- **Case studies** — two documented real-world examples of xray catching performance
  regressions on Gadi and Setonix (`docs/case-studies/`).
- **`CHANGELOG.md`** — this file; machine-readable history in Keep a Changelog format,
  maintained from this release onward.

### Changed

- `authors` field in `Cargo.toml` updated to `xray-hpc contributors`.
- VS Code extension bumped to 0.8.0.

---

## [0.7.0] — 2026-02-17

### Added

- **LSP server mode** — `xray lsp` runs a synchronous JSON-RPC 2.0 Language Server
  over stdin/stdout; no async runtime required.
  - Handles `initialize`, `initialized`, `textDocument/didOpen`, `textDocument/didSave`,
    `textDocument/didClose`, `shutdown`, and `exit`.
  - Publishes `textDocument/publishDiagnostics` after every open/save event.
  - `codeDescription.href` populated from each rule's docs URL.
- **VS Code extension** — `editors/vscode/` contains `package.json` and `extension.js`.
  - Spawns `xray lsp` as a subprocess; communicates via vscode-languageclient.
  - Settings: `xray.serverPath`, `xray.configFile`, `xray.minSeverity`, `xray.enabled`,
    `xray.trace.server`.
  - Commands: `xray.restartServer`, `xray.showOutput`.
  - Watches `xray.toml` and `.xrayignore` for workspace changes.
- **Watch mode** — `xray --watch` re-lints changed `.py` files on save using
  `notify::RecommendedWatcher`; 50 ms debounce avoids double-fire on atomic writes.
  - Respects `.xrayignore` and `[paths]` config excludes.
  - Prints a separator-bordered change summary to stderr on each lint cycle.
- **Diagnostic URLs** — all 25 rules now carry a `url` pointing to
  `https://github.com/greensh16/xray/rules/<RULE_ID>` for in-editor "more info" links.
  Five previously missing URLs added: DK002, NP002, NP003, NP004, IO004.

### Changed

- `SERVER_VERSION` in `lsp.rs` derives from `CARGO_PKG_VERSION` at compile time.

---

## [0.6.0] — 2026-01-17

### Added

- **GitHub Actions composite action** — `action.yml`; inputs: `paths`, `min-severity`,
  `fail-on`; downloads the binary from releases, runs xray, uploads SARIF to Code
  Scanning.
- **pre-commit hook** — `.pre-commit-hooks.yaml` with two hooks: `xray` (blocking on
  warnings) and `xray-warn-only` (always passes, warnings to stdout).
- **SARIF 2.1.0 output** — `--format sarif` emits a full `tool.driver.rules` array,
  per-result locations, fix objects, and `helpUri` from the rule's docs URL.
- **GitLab Code Quality report** — `--format gitlab-codequality` emits the JSON array
  format expected by GitLab CI; severity mapped to `critical`/`major`/`info`;
  fingerprints computed via FNV-1a (no extra dependency).
- **Diff-aware mode** — `xray --diff <REF>` lints only files changed since the given
  git ref (`--diff-filter=ACMR`; deleted files excluded).
- **Benchmark suite** — `benches/throughput.rs` using Criterion 0.5; tracks
  `bench_lint_fixture`, `bench_parse_only`, and `bench_all_fixtures`.

### Changed

- `OutputFormat` enum extended with `Sarif` and `GitlabCodequality` variants.
- `runner::RunResults` gains `paths: Vec<String>` for SARIF/GitLab consumers.

---

## [0.5.0] — 2025-12-17

### Added

- **`.xrayignore` file** — gitignore-style patterns; bare names expand to `**/name`,
  directory patterns append `/**`. File is discovered by walking up from the project
  root.
- **Per-rule severity overrides** — `[severity_overrides]` section in `xray.toml`
  maps rule IDs to `"hint"`, `"warning"`, or `"error"`.
- **`[paths]` config section** — `include` and `exclude` glob lists; default include
  is `["**/*.py"]`.
- **Environment variable support** — `XRAY_CONFIG`, `XRAY_FORMAT`,
  `XRAY_MIN_SEVERITY`, `XRAY_DISABLE` as fallbacks for all major CLI options.
- **Config validation** — `Config::validate()` emits clear errors for unknown rule
  IDs in `disable`, bad severity strings, or zero threshold values.

### Changed

- `Config` struct extended with `severity_overrides: HashMap<String, String>` and
  `paths: PathsConfig`.
- `xray init` template now includes commented `[severity_overrides]` and `[paths]`
  sections.

---

## [0.4.0] — 2025-11-17

### Added

- **`xray explain <RULE_ID>`** — prints rule rationale, bad/good code examples, and
  a link to relevant documentation; implemented for all 25 rules.
- **`xray init`** — scaffolds an annotated `xray.toml` in the current directory with
  all options commented out.
- **Auto-fix suggestions** — `fix_hint: Option<String>` field on `Diagnostic`;
  populated for mechanical fixes (e.g. add `chunks=` to `open_dataset`, replace
  `math.sqrt` with `np.sqrt`); surfaced in text and JSON output.
- **`--stats` flag** — per-rule and per-file summary table printed after linting.
- **Shell completions** — `xray completions <SHELL>` generates completion scripts
  for bash, zsh, and fish via clap_complete.
- **Exit code documentation** — codes 0 (clean), 1 (diagnostics found), 2 (fatal
  error) stabilised.

### Changed

- CLI refactored to clap subcommands: `explain`, `init`, `completions`; bare
  invocation still lints.
- `Cli` struct replaces `Args`; `XrayCommand` enum added.

---

## [0.3.0] — 2025-10-17

### Added

- **XR006** — `ds.to_array()` without `dim=` creates an unnamed concatenation
  dimension, causing silent downstream breakage.
- **XR007** — `xr.concat` inside a loop (O(n²) memory growth, same class of issue
  as NP002).
- **DK005** — `.persist()` result is never reused in the same scope; the persist
  call is wasted work.
- **DK006** — `.compute()` and `.persist()` mixed on the same graph in the same
  scope; graph is materialised twice.
- **NP006** — `np.matrix` usage flagged as deprecated; `np.ndarray` recommended.
- **NP007** — `DataFrame.applymap` / `Series.apply` with a Python lambda inside a
  loop; vectorised alternatives recommended.
- **IO005** — `h5py.File` opened without `swmr=True` in a parallel context.
- **IO006** — `xr.open_dataset(engine="scipy")` on files whose size exceeds a
  configurable threshold.

### Changed

- Total rule count: 17 → 25.
- Rule coverage documentation expanded with one worked example per new rule.

---

## [0.2.0] — 2025-09-17

### Added

- **Inline suppression comments** — `# xray: disable=XR001` suppresses the
  diagnostic on that line; `# xray: disable-file=XR001` suppresses the rule for
  the entire file.
- **AST-based import detection** — replaced substring matching with proper
  tree-sitter import-node traversal; eliminates false triggers from string literals
  and comments that mention library names.
- **Scope-aware XR002** — `.values` method-call guard; `dict.values()` and
  `set.values()` no longer trigger; only bare property access is flagged.
- **NP003 dtype detection hardening** — `dtype=` matched as an actual keyword
  argument AST node rather than a substring of the argument list.
- **NP004 scope expansion** — `math.*` scalar functions flagged everywhere; warning
  inside a for loop, hint when called outside one.
- **Fatal query compilation** — `.scm` syntax errors panic at startup rather than
  silently returning empty results.

---

## [0.1.0] — 2025-08-17

### Added

- **Core linting engine** — tree-sitter AST parsing with zero Python runtime
  dependency.
- **17 rules** across four domains:
  - xarray: XR001–XR005
  - dask: DK001–DK004
  - NumPy/pandas: NP001–NP005
  - scientific I/O: IO001–IO004
- **CLI** with `--format` (text/json), `--min-severity`, `--disable`, `--list-rules`.
- **TOML configuration** (`xray.toml`) with per-domain knobs and threshold settings.
- **Parallel file processing** via rayon.
- **Integration test suite** with clean/bad fixture files for all rule domains.

---

[Unreleased]: https://github.com/greensh16/xray/compare/v1.1.0...HEAD
[1.1.0]: https://github.com/greensh16/xray/compare/v1.0.1...v1.1.0
[1.0.1]: https://github.com/greensh16/xray/compare/v1.0.0...v1.0.1
[1.0.0]: https://github.com/greensh16/xray/compare/v0.9.0...v1.0.0
[0.9.0]: https://github.com/greensh16/xray/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/greensh16/xray/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/greensh16/xray/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/greensh16/xray/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/greensh16/xray/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/greensh16/xray/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/greensh16/xray/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/greensh16/xray/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/greensh16/xray/releases/tag/v0.1.0
