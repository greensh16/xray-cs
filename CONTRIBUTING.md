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

- Report a false positive or false negative by [opening an issue](https://github.com/greensh16/xray/issues/new/choose).
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
git clone https://github.com/greensh16/xray.git
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

```
xray/
├── src/
│   ├── main.rs          # CLI entry point — dispatches subcommands
│   ├── cli.rs           # clap CLI definition (Cli, XrayCommand, OutputFormat)
│   ├── config.rs        # xray.toml schema + Config::from_dir(), validate()
│   ├── parser.rs        # tree-sitter parse_file() and parse_source()
│   ├── diagnostic.rs    # Diagnostic struct, Severity enum
│   ├── rules/
│   │   ├── mod.rs       # run_all() — runs every enabled rule domain
│   │   ├── xarray.rs    # XR001–XR007
│   │   ├── dask.rs      # DK001–DK006
│   │   ├── numpy.rs     # NP001–NP007
│   │   └── io.rs        # IO001–IO006
│   ├── runner.rs        # File collection, parallel lint, output formatting
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
├── docs/                # Documentation site source (Markdown)
└── editors/
    └── vscode/          # VS Code extension
```

### Data flow for a single file

```
CLI args / env vars
    │
    ▼
Config::from_dir()          ← loads xray.toml, .xrayignore
    │
    ▼
parser::parse_file()        ← tree-sitter → ParsedFile { tree, source, path }
    │
    ▼
rules::run_all()            ← runs each domain's check_*() fns
    │   each fn executes a tree-sitter query and produces Vec<Diagnostic>
    ▼
runner::format_output()     ← text / JSON / SARIF / GitLab CQ
    │
    ▼
stdout / exit code
```

---

## Adding a New Rule

### Step 1: Assign an ID

Rules are grouped by prefix:

| Prefix | Domain               | Current range |
|--------|----------------------|---------------|
| XR     | xarray               | XR001–XR007   |
| DK     | dask                 | DK001–DK006   |
| NP     | NumPy / pandas       | NP001–NP007   |
| IO     | Scientific I/O       | IO001–IO006   |

The next available ID in each group is the one after the highest existing number.
If you are adding a rule to a new domain, propose a new prefix in your issue first.

### Step 2: Write the tree-sitter query

Rules are implemented as tree-sitter queries in the `src/rules/<domain>.rs` file.
See [Writing Tree-sitter Queries](#writing-tree-sitter-queries) for details.

### Step 3: Implement `check_<rule_id>()`

Each rule is a standalone function that takes a `&ParsedFile` and returns
`Vec<Diagnostic>`. Add yours to the appropriate `src/rules/<domain>.rs`:

```rust
pub fn check_xr008(parsed: &ParsedFile) -> Vec<Diagnostic> {
    // Query string — matches the bad pattern
    let query_src = r#"
        (call
          function: (attribute
            object: (_) @obj
            attribute: (identifier) @method (#eq? @method "your_bad_method"))
          arguments: (argument_list) @args)
    "#;

    let query = Query::new(&tree_sitter_python::LANGUAGE.into(), query_src)
        .expect("XR008: invalid query");

    let mut cursor = QueryCursor::new();
    let mut diags = Vec::new();

    for m in cursor.matches(&query, parsed.tree.root_node(), parsed.source.as_bytes()) {
        let node = m.captures[0].node; // the anchor node for line/column
        diags.push(
            Diagnostic::new(
                "XR008",
                Severity::Warning,
                &parsed.path,
                node.start_position().row + 1,  // 1-based
                node.start_position().column + 1,
                "your_bad_method() causes X — use alternative_method() instead",
            )
            .with_url("https://github.com/greensh16/xray/wiki/xarray-Rules#xr008")
            .with_fix("Replace your_bad_method() with alternative_method()")
        );
    }

    diags
}
```

### Step 4: Register the rule

In `src/rules/mod.rs`, add your function to `run_all()`:

```rust
pub fn run_all(parsed: &ParsedFile, path: &str, config: &Config) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    // … existing calls …
    if !config.disable.contains("XR008") {
        diags.extend(xarray::check_xr008(parsed));
    }
    diags
}
```

Also add the rule ID to `all_meta()` which is used to populate the SARIF
`tool.driver.rules` array and the `--list-rules` output.

### Step 5: Add config knobs (optional)

If your rule has a configurable threshold (e.g. a file-size limit like IO006),
add a field to the relevant domain config struct in `src/config.rs`:

```rust
pub struct XarrayConfig {
    // … existing fields …
    pub your_threshold: u64,
}
```

Set a sensible default and document the key in `src/init.rs` template and
`docs/configuration.md`.

### Step 6: Add an `explain` entry

Open `src/explain.rs` and add an `ExplainEntry` for your rule:

```rust
ExplainEntry {
    id: "XR008",
    severity: "warning",
    title: "your_bad_method() causes X",
    why: "Explain the underlying reason why this pattern is slow or incorrect \
          on HPC systems. Be concrete — mention memory layout, scheduler overhead, \
          etc.",
    bad: r#"
# Bad — triggers XR008
result = ds.your_bad_method()
"#,
    good: r#"
# Good — use alternative_method() instead
result = ds.alternative_method()
"#,
    references: &[
        "https://github.com/greensh16/xray/wiki/xarray-Rules#xr008",
    ],
},
```

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

Add a `#[cfg(test)]` block at the bottom of the rule file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_source;

    #[test]
    fn xr008_triggers_on_bad_method() {
        let src = "import xarray as xr\nresult = ds.your_bad_method()\n";
        let parsed = parse_source(src.to_string()).unwrap();
        let diags = check_xr008(&parsed);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule_id, "XR008");
    }

    #[test]
    fn xr008_clean_does_not_trigger() {
        let src = "import xarray as xr\nresult = ds.alternative_method()\n";
        let parsed = parse_source(src.to_string()).unwrap();
        let diags = check_xr008(&parsed);
        assert!(diags.is_empty());
    }
}
```

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
- [ ] `cargo clippy -- -D warnings` produces no warnings.
- [ ] `cargo fmt --check` reports no formatting issues.
- [ ] New rule has both a unit test and a fixture-based integration test.
- [ ] `docs/rules/<domain>.md` updated with the new rule section.
- [ ] `docs/rules/index.md` table updated.
- [ ] `src/explain.rs` entry added.
- [ ] `CHANGELOG.md` updated under `[Unreleased]`.
- [ ] PR description links to the originating issue (if one exists).

For non-trivial changes, please open an issue or discussion before investing
significant time — it saves everyone effort if the direction needs adjustment.
