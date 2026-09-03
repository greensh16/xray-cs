# Contributing to xray

Thank you for wanting to improve xray! This guide walks through the full
lifecycle of a contribution — from filing an idea to merging a new rule.

---

## Table of Contents

1. [Code of Conduct](#code-of-conduct)
2. [Ways to Contribute](#ways-to-contribute)
3. [Development Setup](#development-setup)
4. [Architecture Overview](#architecture-overview)
5. [Adding a New Rule](#adding-a-new-rule)
6. [Writing Tree-sitter Queries](#writing-tree-sitter-queries)
7. [Writing Tests](#writing-tests)
8. [Updating Documentation](#updating-documentation)
9. [Pull Request Checklist](#pull-request-checklist)

---

## Code of Conduct

xray is used by scientists who are not professional software engineers. Please
be patient, welcoming, and constructive in all interactions. We follow the
[Contributor Covenant v2.1](https://www.contributor-covenant.org/version/2/1/code_of_conduct/).

---

## Ways to Contribute

**No code required:**

- Report a false positive or false negative by [opening an issue](https://github.com/greensh16/xray-cs/issues/new/choose).
- Propose a new rule using the [Rule Request template](.github/ISSUE_TEMPLATE/rule-request.md).
- Improve documentation — fix a typo, add an example, or clarify a config option.
- Share a case study: a real HPC script where xray caught a performance issue.

**Code contributions:**

- Fix a bug in an existing rule's tree-sitter query.
- Implement a rule that has been triaged and marked `help-wanted`.
- Add integration tests for edge cases.
- Improve the CLI, config parser, output formatters, or LSP server.

---

## Development Setup

### Prerequisites

- **Rust** 1.85 or later (`rustup update stable`)
- **Git**
- A C toolchain for tree-sitter's C parser (`gcc` / `clang` / MSVC)

### Clone and build

```bash
git clone https://github.com/greensh16/xray-cs.git
cd xray
cargo build
```

### Run the test suite

```bash
cargo test
```

All tests should pass on the first run. If something is broken please open an
issue before proceeding.

### Run with a local Python file

```bash
cargo run -- path/to/analysis.py
```

---

## Architecture Overview

xray is **purely syntactic**: tree-sitter parses the file, rules fire on API
names, and nothing resolves types or runs Python. Anything needing runtime shape
or dtype information is out of scope by design.

```
xray/
├── queries/             # ONE .scm file per domain — the rule patterns live here
│   ├── xarray.scm       # XR001–XR011, in pattern order
│   ├── dask.scm         # DK001–DK009
│   ├── numpy.scm        # NP001–NP007
│   └── io.scm           # IO001–IO006
├── src/
│   ├── main.rs          # CLI entry point — dispatches subcommands
│   ├── cli.rs           # clap CLI definition (Cli, XrayCommand, OutputFormat)
│   ├── config.rs        # xray.toml schema + Config::from_dir(), validate()
│   ├── parser.rs        # parse_file()/parse_source(), ImportContext, Suppressions
│   ├── bindings.rs      # intra-function receiver tracking (what a name was assigned from)
│   ├── diagnostic.rs    # Diagnostic struct, Severity enum, RuleMeta
│   ├── rules/
│   │   ├── mod.rs       # RuleSet trait, run_all(), all_meta(), redundancy filter
│   │   ├── xarray.rs    # XR001–XR011
│   │   ├── dask.rs      # DK001–DK009
│   │   ├── numpy.rs     # NP001–NP007
│   │   └── io.rs        # IO001–IO006
│   ├── notebook.rs      # .ipynb cell extraction
│   ├── runner.rs        # File collection, parallel lint, output rendering
│   ├── explain.rs       # xray explain — ExplainEntry per rule
│   ├── init.rs          # xray init — writes xray.toml template
│   ├── ignore.rs        # .xrayignore pattern engine
│   ├── diff.rs          # git diff integration for --diff mode
│   ├── lsp.rs           # Synchronous LSP server (xray lsp)
│   └── watch.rs         # File-watch mode (xray --watch)
├── tests/
│   ├── fixtures/        # Clean and bad Python files used by integration tests
│   └── integration_tests.rs
├── benches/
│   └── throughput.rs    # Criterion benchmarks
└── docs/                # Documentation site source (Markdown)
```

### The rule dispatch contract

This is the most important thing to understand before adding a rule.

Each domain has **one** `.scm` file, pulled in with `include_str!` and compiled
into **one** tree-sitter `Query` (once per process, in a `LazyLock`).
`RuleSet::check` walks the matches and dispatches on `m.pattern_index` — the
0-based position of that pattern in the `.scm` file:

```
queries/xarray.scm pattern 0 → XR001
                   pattern 1 → XR002
                   pattern 2 → XR003 …
```

> **Inserting or reordering a pattern in a `.scm` file silently reassigns every
> later rule.** Always append new patterns at the end of the file, and add the
> matching `match` arm at the end of `check()`.

The query does the coarse structural match; the Rust arm does the refinement
using helpers from `src/parser.rs` (`has_keyword_arg`, `keyword_arg_value`,
`is_inside_loop`, `call_is_from`, `node_text`, `position`). Capture nodes are
fetched **by name, not index**:

```rust
query
    .capture_index_for_name("xr_open_call")
    .and_then(|i| m.nodes_for_capture_index(i).next())
```

Not every rule is one-match-one-diagnostic: DK003 uses its pattern purely to
*count* `.compute()` calls, then emits a single diagnostic after the loop.

Files are linted in parallel with rayon, so rule code must be free of shared
mutable state.

### Data flow for a single file

```
CLI args / env vars
    │
    ▼
Config::from_dir()          ← walks up for xray.toml; CLI/env override
    │
    ▼
path collection             ← --diff ref > positional paths > [paths].include,
    │                         then [paths].exclude globs, then .xrayignore
    ▼
parser::parse_file()        ← tree-sitter → ParsedFile {
    │                             source, tree, imports, suppressions, bindings
    │                         }
    ▼
rules::run_all()            ← each domain's RuleSet::check, gated on imports
    │
    ▼
suppression + sort          ← inline `# xray: disable=` / `disable-file=`
    │
    ▼
apply_filters()             ← severity_overrides → --disable → --min-severity
    │
    ▼
render_text / _json / _sarif / _gitlab
    │
    ▼
stdout / exit code          ← non-zero governed by --fail-on (default: error)
```

---

## Adding a New Rule

### Step 1: Assign an ID

Rules are grouped by prefix:

| Prefix | Domain               | Current range |
|--------|----------------------|---------------|
| XR     | xarray               | XR001–XR011   |
| DK     | dask                 | DK001–DK009   |
| NP     | NumPy / pandas       | NP001–NP007   |
| IO     | Scientific I/O       | IO001–IO006   |

The next available ID in each group is the one after the highest existing number.
Rule IDs are frozen once released — never renumber. If you are adding a rule to a
new domain, propose a new prefix in your issue first.

### Step 2: Append the query pattern

Queries live in `queries/<domain>.scm`, **not** inline in the Rust source.
Append your pattern to the end of the file with a leading `; ID — rationale`
comment, matching the existing convention:

```scheme
; XR012 — .chunk() called with a literal chunk size of 1 on a dimension.
; A chunk of 1 produces one task per index — millions of tiny reads on Lustre.
(call
  function: (attribute
    attribute: (identifier) @xr_chunk_method
    (#eq? @xr_chunk_method "chunk")
  )
  arguments: (argument_list) @xr_chunk_args
) @xr_chunk_call
```

Appending matters: the pattern's position in the file *is* its `pattern_index`,
so inserting one in the middle silently reassigns every rule after it.

See [Writing Tree-sitter Queries](#writing-tree-sitter-queries) for how to
develop the pattern itself.

### Step 3: Add the `match` arm

Add a `RuleMeta` entry to the domain's `meta()` and a `match` arm at the **end**
of `check()`, numbered to match your pattern's position in the `.scm` file:

```rust
// XR012 — chunk size of 1
11 if !config.is_disabled("XR012") => {
    if let Some(call_node) = query
        .capture_index_for_name("xr_chunk_call")
        .and_then(|i| m.nodes_for_capture_index(i).next())
    {
        // The query matched the shape; refine it here.
        if !call_is_from(call_node, source, &file.imports, "xarray") {
            continue;
        }
        let (line, col) = position(&call_node);
        diags.push(
            Diagnostic::new(
                "XR012",
                Severity::Warning,
                path,
                line,
                col,
                "chunk size of 1 creates one task per index",
            )
            .with_suggestion("Choose a chunk size matching your storage layout")
            .with_fix_hint("chunks={'time': 24}")
            .with_url("https://github.com/greensh16/xray-cs/wiki/xarray-Rules#xr012"),
        );
    }
}
```

Points worth noting:

- Guard every arm with `!config.is_disabled("ID")`.
- Fetch captures **by name**, never `m.captures[0]`.
- `position(&node)` returns 1-based `(line, column)`.
- The builder is `.with_suggestion()`, `.with_fix_hint()`, `.with_url()`.
- If your rule is loop-sensitive, use `is_inside_loop(node)` in Rust — never
  express loop context in the query.
- If your rule inspects a receiver, use `file.bindings.origin_of(...)` and treat
  an unknown origin as *keep firing*, never as *safe*.

### Step 4: Check the import gate

`rules::run_all` only runs a domain when the matching flag on
`ParsedFile.imports` is set. If your rule targets a library not already in
`ImportContext` (`src/parser.rs`), add a field there and a `mark_by_name` arm —
otherwise the rule can never fire, no matter how good the query is.

`all_meta()` in `src/rules/mod.rs` collects every domain's `meta()`, and feeds
both `--list-rules` and the SARIF `tool.driver.rules` array. Adding your
`RuleMeta` in Step 3 is enough; there is no separate registration call.

### Step 5: Add config knobs (optional)

If your rule has a configurable threshold, add a field to the relevant domain
config struct in `src/config.rs`. Every section uses
`#[serde(deny_unknown_fields)]`, so the field must carry a `#[serde(default)]`
or existing config files without the key will fail to load:

```rust
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XarrayConfig {
    // … existing fields …
    #[serde(default = "default_your_threshold")]
    pub your_threshold: u64,
}
```

A new config key must be added in **three** places or it will drift:

1. the struct in `src/config.rs` (with a default, and a `Default` impl entry),
2. the annotated template in `src/init.rs`,
3. `docs/configuration.md`.

Add a unit test that flips the key — see `np001_respects_the_config_toggle` in
`src/rules/numpy.rs`.

### Step 6: Add an `explain` entry

`xray explain <ID>` must work for every rule. Open `src/explain.rs` and add an
`ExplainEntry`:

```rust
ExplainEntry {
    id: "XR012",
    name: "chunk-size-one",
    severity: "warning",
    domain: "xarray",
    rationale: "\
Explain the underlying reason this pattern is slow or wrong on HPC systems.
Be concrete — mention memory layout, scheduler overhead, or storage behaviour.
If the rule deliberately does not fire in some cases, say so here.",
    bad_example: "\
ds = ds.chunk({\"time\": 1})   # one task per timestep",
    good_example: "\
ds = ds.chunk({\"time\": 24})  # matches the on-disk chunk layout",
    url: Some("https://github.com/greensh16/xray-cs/wiki/xarray-Rules#xr012"),
    fix_eligible: false,
},
```

`fix_eligible` marks rules with a safe mechanical rewrite, for the planned
`xray fix` subcommand.

---

## Writing Tree-sitter Queries

xray uses [tree-sitter-python](https://github.com/tree-sitter/tree-sitter-python)
to parse Python source into a concrete syntax tree, then matches patterns with
S-expression queries.

### Useful tools

- **tree-sitter CLI** — `npm i -g tree-sitter-cli` then `tree-sitter parse file.py`
  to inspect the CST of any Python snippet.
- **Playground** — https://tree-sitter.github.io/tree-sitter/playground lets you
  write queries interactively.

### Key node types for HPC patterns

```scheme
; Function call: foo(a, b)
(call function: (_) @fn arguments: (argument_list) @args)

; Attribute access / method call: obj.method(...)
(call
  function: (attribute
    object: (_) @obj
    attribute: (identifier) @attr))

; Keyword argument: func(key=value)
(keyword_argument name: (identifier) @key value: (_) @val)

; For loop body
(for_statement body: (block (_)* @stmt))

; Import: import xarray as xr
(import_statement name: (dotted_name) @name)

; Aliased import: import xarray as xr
(aliased_import name: (dotted_name) @name alias: (identifier) @alias)
```

### Predicates

| Predicate | Example | Meaning |
|-----------|---------|---------|
| `(#eq? @node "literal")` | `(#eq? @method "compute")` | Node text equals literal |
| `(#match? @node "^(open\|load)$")` | — | Node text matches regex |
| `(#not-eq? @node "text")` | — | Negation of `#eq?` |

### Guard against false positives

Always check that the relevant import is present before emitting diagnostics.
`ParsedFile` stores an `imports: HashSet<String>` populated by the parser.
Check it at the top of your `check_*` function:

```rust
if !parsed.imports.contains("xarray") && !parsed.imports.contains("xr") {
    return vec![];
}
```

---

## Writing Tests

### Unit tests (inline)

Every rule file has a `#[cfg(test)]` block at the bottom. Add your cases to the
existing one — each rule gets a test asserting it fires **and** that it stays
silent on the nearest legitimate lookalike.

There is no per-rule `check_*` function: call the domain's `RuleSet::check`
directly. Doing so deliberately bypasses `rules::run_all`, so import gating and
the cross-domain redundancy filter are not in the way and a failure points at
one rule set.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_source;

    /// Rule IDs fired by `src`, in line order.
    fn ids(src: &str) -> Vec<&'static str> {
        let parsed = parse_source(src.to_string()).unwrap();
        let mut diags = XarrayRules::check(&parsed, "<test>", &Config::default());
        diags.sort_by_key(|d| (d.line, d.rule_id));
        diags.into_iter().map(|d| d.rule_id).collect()
    }

    fn fires(rule: &str, src: &str) -> bool {
        ids(src).iter().any(|id| *id == rule)
    }

    #[test]
    fn xr008_open_mfdataset_without_parallel() {
        assert!(fires(
            "XR008",
            "import xarray as xr\nds = xr.open_mfdataset('*.nc', chunks='auto')\n"
        ));
        assert!(!fires(
            "XR008",
            "import xarray as xr\nds = xr.open_mfdataset('*.nc', chunks='auto', parallel=True)\n"
        ));
    }
}
```

If the rule reads a config key, add a test that flips it — see
`np001_respects_the_config_toggle` in `src/rules/numpy.rs`.

If the rule inspects a receiver, `src/bindings.rs` resolves what a name was
assigned from. Treat an unknown origin as "keep the previous behaviour", never
as "safe": function parameters are always unknown, so the alternative silently
drops real diagnostics.

### Integration tests (fixture-based)

Add fixture files under `tests/fixtures/`:

```
tests/fixtures/xr008_bad.py    ← must produce exactly one XR008 diagnostic
tests/fixtures/xr008_clean.py  ← must produce zero diagnostics
```

Then add test cases in `tests/integration_tests.rs`:

```rust
#[test]
fn xr008_bad_fixture_triggers() {
    let diags = lint_fixture("xr008_bad.py");
    assert!(diags.iter().any(|d| d.rule_id == "XR008"));
}

#[test]
fn xr008_clean_fixture_is_silent() {
    let diags = lint_fixture("xr008_clean.py");
    assert!(diags.iter().all(|d| d.rule_id != "XR008"));
}
```

### Running tests

```bash
# All tests
cargo test

# One specific test
cargo test xr008

# With output (useful for debugging)
cargo test xr008 -- --nocapture
```

---

## Updating Documentation

For every new rule, update or create:

1. **`docs/rules/<domain>.md`** — add a section following the existing format:
   rule ID, title, severity, rationale, bad/good examples, config knobs (if any),
   and a "Why is this slow on HPC?" explainer.

2. **`docs/rules/index.md`** — add a row to the rule reference table.

3. **`src/explain.rs`** — add an `ExplainEntry` (Step 6 above).

4. **`CHANGELOG.md`** — add an entry under `[Unreleased]`.

---

## Pull Request Checklist

Before submitting, verify:

- [ ] `cargo test` passes with no failures.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` produces no warnings.
      This is the exact command the CI lint job runs; the bare `cargo clippy` form skips
      tests, benches and the bin target. CI resolves `stable` fresh, so run
      `rustup update stable` first — a lint added in a newer release will fail CI even
      though your local run was clean.
- [ ] `cargo fmt --check` reports no formatting issues.
- [ ] New rule has both a unit test and a fixture-based integration test.
- [ ] `docs/rules/<domain>.md` updated with the new rule section.
- [ ] `docs/rules/index.md` table updated.
- [ ] `src/explain.rs` entry added.
- [ ] `CHANGELOG.md` updated under `[Unreleased]`.
- [ ] PR description links to the originating issue (if one exists).

For non-trivial changes, please open an issue or discussion before investing
significant time — it saves everyone effort if the direction needs adjustment.
