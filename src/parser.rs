use anyhow::Result;
use std::collections::{HashMap, HashSet};
use tree_sitter::{Node, Parser, Tree};

/// A parsed Python source file ready for rule inspection.
pub struct ParsedFile {
    pub source: String,
    pub tree: Tree,
    pub imports: ImportContext,
    pub suppressions: Suppressions,
}

/// Which scientific libraries are imported in this file — used to
/// gate rules so we only flag e.g. xarray patterns if xarray is present.
#[derive(Debug, Default, Clone)]
pub struct ImportContext {
    pub xarray: bool,
    pub dask: bool,
    pub numpy: bool,
    pub pandas: bool,
    pub netcdf4: bool,
    pub zarr: bool,
    pub h5py: bool,
    /// Binding name → canonical top-level module.
    /// `import xarray as xr` → `"xr" → "xarray"`;
    /// `import dask.array as dsa` → `"dsa" → "dask"`;
    /// `import numpy` → `"numpy" → "numpy"`.
    ///
    /// Rules used to hard-code the conventional aliases (`xr`, `np`, `da`,
    /// `pd`), which both missed unconventional aliases and mis-attributed
    /// calls on unrelated objects.
    pub aliases: HashMap<String, String>,
    /// Name bound by `from <module> import <name>` → that module.
    /// `from xarray import concat` → `"concat" → "xarray"`.
    pub from_imports: HashMap<String, String>,
}

impl ImportContext {
    /// The module a receiver identifier refers to, if it is an imported alias.
    pub fn module_of_binding(&self, binding: &str) -> Option<&str> {
        self.aliases.get(binding).map(String::as_str)
    }

    /// The module a bare name was imported from, if any.
    pub fn module_of_name(&self, name: &str) -> Option<&str> {
        self.from_imports.get(name).map(String::as_str)
    }

    /// Scan the top-level import statements using AST node traversal to build
    /// the context. This avoids false positives from string literals or
    /// comments that happen to contain library names.
    fn from_tree(root: Node<'_>, source: &[u8]) -> Self {
        let mut ctx = ImportContext::default();

        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            match child.kind() {
                // `import xarray` or `import xarray as xr` or `import dask.array as da`
                "import_statement" => {
                    let mut c = child.walk();
                    for name_node in child.children(&mut c) {
                        // The binding introduced into the local namespace:
                        // the alias if there is one, otherwise the first
                        // component of the dotted path (`import dask.array`
                        // binds `dask`).
                        let (module_root, binding) = match name_node.kind() {
                            "dotted_name" => (name_node, None),
                            "aliased_import" => {
                                let Some(n) = name_node.child_by_field_name("name") else {
                                    continue;
                                };
                                let alias = name_node
                                    .child_by_field_name("alias")
                                    .map(|a| node_text(&a, source));
                                (n, alias)
                            }
                            _ => continue,
                        };
                        // Only look at the leading identifier of the dotted path
                        if let Some(first) = module_root.child(0)
                            && first.kind() == "identifier"
                        {
                            let name = node_text(&first, source);
                            Self::mark_by_name(&mut ctx, name);
                            let binding = binding.unwrap_or(name);
                            ctx.aliases.insert(binding.to_string(), name.to_string());
                        }
                    }
                }
                // `from xarray import DataArray` or `from dask.array import from_delayed`
                "import_from_statement" => {
                    if let Some(module_node) = child.child_by_field_name("module_name") {
                        // First identifier in the dotted module path
                        let Some(first) = module_node.child(0) else {
                            continue;
                        };
                        if first.kind() != "identifier" {
                            continue;
                        }
                        let module = node_text(&first, source);
                        Self::mark_by_name(&mut ctx, module);

                        // Record every name this statement binds, so a bare
                        // `concat(...)` can be traced back to xarray.
                        let mut nc = child.walk();
                        for imported in child.children(&mut nc) {
                            if imported.id() == module_node.id() {
                                continue;
                            }
                            match imported.kind() {
                                "dotted_name" => {
                                    if let Some(n) = imported.child(0) {
                                        ctx.from_imports.insert(
                                            node_text(&n, source).to_string(),
                                            module.to_string(),
                                        );
                                    }
                                }
                                "aliased_import" => {
                                    if let Some(a) = imported.child_by_field_name("alias") {
                                        ctx.from_imports.insert(
                                            node_text(&a, source).to_string(),
                                            module.to_string(),
                                        );
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        ctx
    }

    fn mark_by_name(ctx: &mut Self, module: &str) {
        match module {
            "xarray" => ctx.xarray = true,
            "dask" => ctx.dask = true,
            "numpy" => ctx.numpy = true,
            "pandas" => ctx.pandas = true,
            "netCDF4" | "netcdf4" => ctx.netcdf4 = true,
            "zarr" => ctx.zarr = true,
            "h5py" => ctx.h5py = true,
            _ => {}
        }
    }
}

/// Per-file and per-line inline suppression state, built from
/// `# xray: disable=RULE_ID` and `# xray: disable-file=RULE_ID` comments.
#[derive(Debug, Default)]
pub struct Suppressions {
    /// Rules suppressed for the entire file
    pub file_level: HashSet<String>,
    /// Rules suppressed on a specific 1-based line number
    pub line_level: HashMap<usize, HashSet<String>>,
}

impl Suppressions {
    /// Returns true if `rule_id` is suppressed, either file-wide or on `line`.
    pub fn is_suppressed(&self, rule_id: &str, line: usize) -> bool {
        self.file_level.contains(rule_id)
            || self
                .line_level
                .get(&line)
                .is_some_and(|s| s.contains(rule_id))
    }

    fn from_source(source: &str) -> Self {
        let mut s = Suppressions::default();
        for (i, line) in source.lines().enumerate() {
            let line_num = i + 1;
            // Look for `# xray:` anywhere on the line (allows inline comments)
            if let Some(pos) = line.find("# xray:") {
                let after = line[pos + 7..].trim_start(); // text after "# xray:"
                if let Some(rules_str) = after.strip_prefix("disable-file=") {
                    // File-wide: # xray: disable-file=XR001,XR002
                    for rule in rules_str
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        s.file_level.insert(rule.to_string());
                    }
                } else if let Some(rules_str) = after.strip_prefix("disable=") {
                    // Line-level: # xray: disable=XR001
                    for rule in rules_str
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        s.line_level
                            .entry(line_num)
                            .or_default()
                            .insert(rule.to_string());
                    }
                }
            }
        }
        s
    }
}

pub fn parse_file(path: &str) -> Result<ParsedFile> {
    // Read raw bytes so we handle non-ASCII path characters on all platforms
    // and gracefully recover from non-UTF-8 bytes (e.g. latin-1 comments)
    // by replacing them with the UTF-8 replacement character rather than
    // returning a hard error.
    let bytes = std::fs::read(path).map_err(|e| anyhow::anyhow!("cannot read {path}: {e}"))?;
    let source = String::from_utf8_lossy(&bytes).into_owned();
    parse_source(source)
}

pub fn parse_source(source: String) -> Result<ParsedFile> {
    // Normalise Windows CRLF line endings to LF so that:
    //  1. tree-sitter row numbers match our 1-based line numbers.
    //  2. Suppression comment scanning via `str::lines()` behaves correctly.
    // This is a no-op on files that already use LF.
    let source = source.replace("\r\n", "\n");
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_python::LANGUAGE.into())?;

    let tree = parser
        .parse(&source, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter failed to produce a parse tree"))?;

    let imports = ImportContext::from_tree(tree.root_node(), source.as_bytes());
    let suppressions = Suppressions::from_source(&source);

    Ok(ParsedFile {
        source,
        tree,
        imports,
        suppressions,
    })
}

/// Convenience: get 1-based (line, col) from a tree-sitter node
pub fn position(node: &Node<'_>) -> (usize, usize) {
    let p = node.start_position();
    (p.row + 1, p.column + 1)
}

/// Extract the UTF-8 text of a node
pub fn node_text<'a>(node: &Node<'_>, source: &'a [u8]) -> &'a str {
    node.utf8_text(source).unwrap_or("<invalid utf8>")
}

/// Returns true if `call_node` has a keyword_argument whose name exactly matches `kw`.
/// Uses AST traversal rather than substring matching to avoid false positives.
pub fn has_keyword_arg(call_node: Node<'_>, source: &[u8], kw: &str) -> bool {
    let mut cursor = call_node.walk();
    for child in call_node.children(&mut cursor) {
        if child.kind() == "argument_list" {
            let mut arg_cursor = child.walk();
            for arg in child.children(&mut arg_cursor) {
                if arg.kind() == "keyword_argument"
                    && let Some(name_node) = arg.child_by_field_name("name")
                    && node_text(&name_node, source) == kw
                {
                    return true;
                }
            }
        }
    }
    false
}

/// Is `node` evaluated repeatedly — i.e. inside the body of a `for` or `while`
/// loop, or inside a comprehension?
///
/// Three cases the previous `for_statement`-only ancestor walk got wrong:
///   * `while` loops were invisible, so `.compute()` in a `while` was missed.
///   * Comprehensions were invisible, so `[x.compute() for x in items]` — the
///     exact anti-pattern these rules exist to catch — was missed.
///   * The loop *header* counted as the loop: `for row in ds.compute():`
///     evaluates the call once, but was reported as a per-iteration compute.
pub fn is_inside_loop(node: Node<'_>) -> bool {
    let mut current = node;
    // Set when we pass through a comprehension's iterable, which is evaluated
    // once rather than per element.
    let mut skip_next_comprehension = false;

    while let Some(parent) = current.parent() {
        match parent.kind() {
            "for_statement" | "while_statement" => {
                // Only the body repeats; `left`/`right`/`alternative` do not.
                if parent
                    .child_by_field_name("body")
                    .is_some_and(|body| body.id() == current.id())
                {
                    return true;
                }
            }
            "for_in_clause" => {
                if parent
                    .child_by_field_name("right")
                    .is_some_and(|right| right.id() == current.id())
                {
                    skip_next_comprehension = true;
                }
            }
            "list_comprehension"
            | "set_comprehension"
            | "dictionary_comprehension"
            | "generator_expression" => {
                if skip_next_comprehension {
                    skip_next_comprehension = false;
                } else {
                    return true;
                }
            }
            _ => {}
        }
        current = parent;
    }
    false
}

/// Left-most identifier of an attribute chain: `dask.array.concatenate` → `dask`.
/// Returns `None` when the chain is rooted in something other than a plain
/// name (`ds.mean().compute()`), which is precisely when a rule cannot tell
/// which library the receiver belongs to.
pub fn attribute_root<'a>(node: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    let mut current = node;
    loop {
        match current.kind() {
            "identifier" => return Some(node_text(&current, source)),
            "attribute" => current = current.child_by_field_name("object")?,
            _ => return None,
        }
    }
}

/// Which library does this call belong to?
///
/// Resolves through the file's import aliases, so `xr.concat(...)`,
/// `xarray.concat(...)` and `from xarray import concat; concat(...)` all
/// answer `"xarray"`, while `pd.concat(...)` answers `"pandas"` and
/// `out.merge(...)` — an unknown receiver — answers `None`.
pub fn call_module<'a>(
    call_node: Node<'_>,
    source: &[u8],
    imports: &'a ImportContext,
) -> Option<&'a str> {
    let func = call_node.child_by_field_name("function")?;
    match func.kind() {
        "identifier" => imports.module_of_name(node_text(&func, source)),
        "attribute" => {
            let root = attribute_root(func.child_by_field_name("object")?, source)?;
            imports.module_of_binding(root)
        }
        _ => None,
    }
}

/// Does `call_node` belong to `module` (e.g. `"xarray"`)?
pub fn call_is_from(
    call_node: Node<'_>,
    source: &[u8],
    imports: &ImportContext,
    module: &str,
) -> bool {
    call_module(call_node, source, imports) == Some(module)
}

/// Does the call forward `**kwargs`?
///
/// When it does, a "missing keyword argument" rule cannot know whether the
/// keyword is present — `xr.open_dataset(path, **opts)` may well set `chunks`
/// — so those rules stay quiet rather than reporting a definite miss.
pub fn has_dictionary_splat(call_node: Node<'_>) -> bool {
    let mut cursor = call_node.walk();
    for child in call_node.children(&mut cursor) {
        if child.kind() == "argument_list" {
            let mut arg_cursor = child.walk();
            for arg in child.children(&mut arg_cursor) {
                if arg.kind() == "dictionary_splat" {
                    return true;
                }
            }
        }
    }
    false
}

/// True when the keyword is present, or when the call forwards `**kwargs` and
/// therefore *might* set it.  Used by every "missing keyword" rule.
pub fn keyword_arg_present_or_unknown(call_node: Node<'_>, source: &[u8], kw: &str) -> bool {
    has_keyword_arg(call_node, source, kw) || has_dictionary_splat(call_node)
}
/// Returns the raw source text of the value of the keyword argument named `kw`
/// in `call_node`, or `None` if no such keyword argument exists.
/// The returned text includes quotes for string literals (e.g. `"scipy"`).
pub fn keyword_arg_value<'a>(call_node: Node<'_>, source: &'a [u8], kw: &str) -> Option<&'a str> {
    let mut cursor = call_node.walk();
    for child in call_node.children(&mut cursor) {
        if child.kind() == "argument_list" {
            let mut arg_cursor = child.walk();
            for arg in child.children(&mut arg_cursor) {
                if arg.kind() == "keyword_argument"
                    && let Some(name_node) = arg.child_by_field_name("name")
                    && node_text(&name_node, source) == kw
                    && let Some(val_node) = arg.child_by_field_name("value")
                {
                    return Some(node_text(&val_node, source));
                }
            }
        }
    }
    None
}
