//! Intra-function receiver tracking.
//!
//! xray is purely syntactic, so it cannot know a variable's type. It can,
//! however, remember what a name was *assigned from* within a single function
//! body, which is enough to answer the one question several rules kept getting
//! wrong: "is this receiver actually an xarray object, or something else that
//! happens to share a method name?"
//!
//! `df.values` and `ds.values` are indistinguishable to a query; they are not
//! indistinguishable once you have seen `df = pd.read_csv(...)` two lines up.
//!
//! Deliberate limits, matching the roadmap's "light inference" scope:
//!
//! - No interprocedural analysis. Function parameters are unknown, which is why
//!   rules must treat `None` as "keep the old behaviour" rather than "safe".
//! - Scopes reset at every `function_definition`; class bodies fold into the
//!   enclosing scope.
//! - A name assigned twice from conflicting origins degrades to unknown rather
//!   than guessing at the last write.

use crate::parser::{ImportContext, node_text};
use std::collections::HashMap;
use tree_sitter::Node;

/// What kind of object a name holds, as far as can be determined syntactically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// `xr.open_dataset(...)`, `xr.Dataset(...)`, a `.sel()` off one of those.
    Xarray,
    /// `da.from_array(...)`, `dask.array.*`.
    Dask,
    /// `pd.read_csv(...)`, `pd.DataFrame(...)`.
    Pandas,
    /// `np.zeros(...)`, `.to_numpy()`, `.values`.
    Numpy,
    /// A literal or builtin container — dict, list, set, tuple, str, number.
    /// Definitely not an array-like, which is what makes it useful.
    Plain,
}

impl Origin {
    /// Does this origin denote an array-like scientific object?
    pub fn is_array_like(self) -> bool {
        !matches!(self, Origin::Plain)
    }
}

#[derive(Debug, Default, Clone)]
struct Scope {
    parent: Option<usize>,
    /// `None` records a name that is bound but whose origin is unknown or
    /// ambiguous — distinct from an absent name, because it must shadow an
    /// outer binding rather than fall through to it.
    names: HashMap<String, Option<Origin>>,
}

/// Origins of the names bound in each scope of one file.
#[derive(Debug, Default, Clone)]
pub struct Bindings {
    /// Keyed by the node id of the scope's `function_definition`, or of the
    /// module root for top-level code.
    scopes: HashMap<usize, Scope>,
    root: usize,
}

impl Bindings {
    /// Walk the tree once, recording what every assignment binds.
    pub fn build(root: Node<'_>, source: &[u8], imports: &ImportContext) -> Self {
        let mut b = Bindings {
            scopes: HashMap::new(),
            root: root.id(),
        };
        b.scopes.insert(root.id(), Scope::default());
        b.walk(root, root.id(), source, imports);
        b
    }

    fn walk(&mut self, node: Node<'_>, scope: usize, source: &[u8], imports: &ImportContext) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "function_definition" {
                let inner = child.id();
                self.scopes.insert(
                    inner,
                    Scope {
                        parent: Some(scope),
                        names: HashMap::new(),
                    },
                );
                // Parameters are deliberately left unrecorded: their origin is
                // genuinely unknown without interprocedural analysis.
                self.walk(child, inner, source, imports);
                continue;
            }

            match child.kind() {
                "assignment" => self.record_assignment(child, scope, source, imports),
                "for_statement" => self.record_targets_unknown(child, "left", scope, source),
                "with_statement" => self.record_with(child, scope, source, imports),
                _ => {}
            }
            self.walk(child, scope, source, imports);
        }
    }

    fn record_assignment(
        &mut self,
        node: Node<'_>,
        scope: usize,
        source: &[u8],
        imports: &ImportContext,
    ) {
        let Some(left) = node.child_by_field_name("left") else {
            return;
        };
        if left.kind() != "identifier" {
            // Tuple / list unpacking: bind every target as unknown so it
            // shadows rather than inherits.
            self.record_targets_unknown(node, "left", scope, source);
            return;
        }
        let origin = node
            .child_by_field_name("right")
            .and_then(|r| self.classify(r, source, imports, scope));
        let name = node_text(&left, source).to_string();
        self.bind(scope, name, origin);
    }

    /// Bind every identifier under `field` as unknown-but-present.
    fn record_targets_unknown(&mut self, node: Node<'_>, field: &str, scope: usize, source: &[u8]) {
        let Some(target) = node.child_by_field_name(field) else {
            return;
        };
        let mut names = Vec::new();
        collect_identifiers(target, source, &mut names);
        for n in names {
            self.bind(scope, n, None);
        }
    }

    /// `with xr.open_dataset(p) as ds:` binds `ds` to the call's origin.
    fn record_with(
        &mut self,
        node: Node<'_>,
        scope: usize,
        source: &[u8],
        imports: &ImportContext,
    ) {
        let mut pairs = Vec::new();
        collect_as_patterns(node, &mut pairs);
        for (value, alias) in pairs {
            let origin = self.classify(value, source, imports, scope);
            let mut names = Vec::new();
            collect_identifiers(alias, source, &mut names);
            for n in names {
                self.bind(scope, n, origin);
            }
        }
    }

    fn bind(&mut self, scope: usize, name: String, origin: Option<Origin>) {
        let entry = self
            .scopes
            .entry(scope)
            .or_default()
            .names
            .entry(name)
            .or_insert(origin);
        // A conflicting rebind degrades to unknown rather than trusting the
        // most recent write — control flow may pick either.
        if *entry != origin {
            *entry = None;
        }
    }

    /// The scope that encloses `node`: the nearest `function_definition`
    /// ancestor, else the module root.
    fn scope_of(&self, node: Node<'_>) -> usize {
        let mut current = node;
        loop {
            if current.kind() == "function_definition" && self.scopes.contains_key(&current.id()) {
                return current.id();
            }
            match current.parent() {
                Some(p) => current = p,
                None => return self.root,
            }
        }
    }

    fn lookup(&self, scope: usize, name: &str) -> Option<Origin> {
        let mut current = Some(scope);
        while let Some(id) = current {
            let s = self.scopes.get(&id)?;
            if let Some(origin) = s.names.get(name) {
                return *origin;
            }
            current = s.parent;
        }
        None
    }

    /// Origin of the object an expression evaluates to, as seen from its own
    /// position in the file. `None` means "cannot tell" — never "not an array".
    pub fn origin_of(
        &self,
        node: Node<'_>,
        source: &[u8],
        imports: &ImportContext,
    ) -> Option<Origin> {
        let scope = self.scope_of(node);
        self.classify(node, source, imports, scope)
    }

    fn classify(
        &self,
        node: Node<'_>,
        source: &[u8],
        imports: &ImportContext,
        scope: usize,
    ) -> Option<Origin> {
        match node.kind() {
            "identifier" => {
                let name = node_text(&node, source);
                // A bare module alias used as a value is not itself an array.
                if let Some(o) = self.lookup(scope, name) {
                    return Some(o);
                }
                None
            }

            "call" => self.classify_call(node, source, imports, scope),

            // `ds.temp` on a Dataset is a DataArray; `df.col` on a DataFrame is
            // a Series. Both keep their library.
            "attribute" => {
                let attr = node
                    .child_by_field_name("attribute")
                    .map(|a| node_text(&a, source));
                let obj = node.child_by_field_name("object")?;
                if matches!(attr, Some("values")) {
                    // `.values` is the numpy escape hatch on every one of these.
                    return match self.classify(obj, source, imports, scope) {
                        Some(o) if o.is_array_like() => Some(Origin::Numpy),
                        other => other,
                    };
                }
                self.classify(obj, source, imports, scope)
            }

            // `ds["temp"]` keeps its library; `d["k"]` on a dict could be
            // anything, so it degrades to unknown.
            "subscript" => {
                let value = node.child_by_field_name("value")?;
                match self.classify(value, source, imports, scope) {
                    Some(Origin::Plain) => None,
                    other => other,
                }
            }

            "binary_operator" => {
                let left = node
                    .child_by_field_name("left")
                    .and_then(|n| self.classify(n, source, imports, scope));
                let right = node
                    .child_by_field_name("right")
                    .and_then(|n| self.classify(n, source, imports, scope));
                match (left, right) {
                    (Some(a), _) if a.is_array_like() => Some(a),
                    (_, Some(b)) if b.is_array_like() => Some(b),
                    _ => None,
                }
            }

            "parenthesized_expression" => {
                let inner = node.named_child(0)?;
                self.classify(inner, source, imports, scope)
            }

            "dictionary"
            | "list"
            | "set"
            | "tuple"
            | "string"
            | "integer"
            | "float"
            | "true"
            | "false"
            | "none"
            | "list_comprehension"
            | "dictionary_comprehension"
            | "set_comprehension"
            | "concatenated_string" => Some(Origin::Plain),

            _ => None,
        }
    }

    fn classify_call(
        &self,
        node: Node<'_>,
        source: &[u8],
        imports: &ImportContext,
        scope: usize,
    ) -> Option<Origin> {
        let func = node.child_by_field_name("function")?;

        match func.kind() {
            // `open_dataset(...)` after `from xarray import open_dataset`.
            "identifier" => {
                let name = node_text(&func, source);
                if BUILTIN_PLAIN.contains(&name) {
                    return Some(Origin::Plain);
                }
                origin_for_module(imports.module_of_name(name)?)
            }

            "attribute" => {
                let method = node
                    .child_by_field_name("function")
                    .and_then(|f| f.child_by_field_name("attribute"))
                    .map(|a| node_text(&a, source));
                let obj = func.child_by_field_name("object")?;

                // `xr.open_dataset(...)`, `np.zeros(...)`: the receiver is a
                // module alias, so the library decides the origin.
                if let Some(root) = crate::parser::attribute_root(obj, source)
                    && let Some(module) = imports.module_of_binding(root)
                    && self.lookup(scope, root).is_none()
                {
                    return origin_for_module(module);
                }

                // Otherwise it is a method on a value we may already know.
                let recv = self.classify(obj, source, imports, scope)?;
                match method {
                    Some("to_numpy") | Some("__array__") => Some(Origin::Numpy),
                    // Computing a dask array yields concrete numpy; computing
                    // an xarray object yields an in-memory xarray object.
                    Some("compute") if recv == Origin::Dask => Some(Origin::Numpy),
                    _ => Some(recv),
                }
            }

            _ => None,
        }
    }
}

/// Builtins whose result is definitely not an array-like.
const BUILTIN_PLAIN: &[&str] = &[
    "dict",
    "list",
    "set",
    "tuple",
    "str",
    "int",
    "float",
    "bool",
    "len",
    "sorted",
    "range",
    "open",
    "zip",
    "enumerate",
];

fn origin_for_module(module: &str) -> Option<Origin> {
    match module {
        "xarray" => Some(Origin::Xarray),
        "dask" => Some(Origin::Dask),
        "pandas" => Some(Origin::Pandas),
        "numpy" => Some(Origin::Numpy),
        _ => None,
    }
}

/// Every identifier in an assignment target, so tuple unpacking shadows all
/// of its names.
fn collect_identifiers(node: Node<'_>, source: &[u8], out: &mut Vec<String>) {
    if node.kind() == "identifier" {
        out.push(node_text(&node, source).to_string());
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_identifiers(child, source, out);
    }
}

/// Collect `(value, alias)` pairs from every `as_pattern` under `node`.
fn collect_as_patterns<'t>(node: Node<'t>, out: &mut Vec<(Node<'t>, Node<'t>)>) {
    if node.kind() == "as_pattern"
        && let Some(value) = node.named_child(0)
        && let Some(alias) = node.child_by_field_name("alias")
    {
        out.push((value, alias));
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_as_patterns(child, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_source;

    fn origins(src: &str, names: &[&str]) -> Vec<Option<Origin>> {
        let file = parse_source(src.to_string()).unwrap();
        let root = file.tree.root_node();
        names
            .iter()
            .map(|n| file.bindings.lookup(root.id(), n))
            .collect()
    }

    #[test]
    fn tracks_module_level_constructors() {
        let got = origins(
            "import xarray as xr\n\
             import pandas as pd\n\
             import numpy as np\n\
             ds = xr.open_dataset('a.nc')\n\
             df = pd.read_csv('a.csv')\n\
             arr = np.zeros(3)\n\
             d = {}\n",
            &["ds", "df", "arr", "d"],
        );
        assert_eq!(
            got,
            vec![
                Some(Origin::Xarray),
                Some(Origin::Pandas),
                Some(Origin::Numpy),
                Some(Origin::Plain),
            ]
        );
    }

    #[test]
    fn resolves_unconventional_aliases() {
        let got = origins(
            "import dask.array as dsa\nx = dsa.from_array([1])\n",
            &["x"],
        );
        assert_eq!(got, vec![Some(Origin::Dask)]);
    }

    #[test]
    fn methods_inherit_the_receivers_library() {
        let got = origins(
            "import xarray as xr\nds = xr.open_dataset('a.nc')\nm = ds.mean()\nv = ds.to_numpy()\n",
            &["m", "v"],
        );
        assert_eq!(got, vec![Some(Origin::Xarray), Some(Origin::Numpy)]);
    }

    #[test]
    fn with_statement_binds_the_alias() {
        let got = origins(
            "import xarray as xr\nwith xr.open_dataset('a.nc') as ds:\n    pass\n",
            &["ds"],
        );
        assert_eq!(got, vec![Some(Origin::Xarray)]);
    }

    #[test]
    fn conflicting_rebind_degrades_to_unknown() {
        let got = origins(
            "import xarray as xr\nimport pandas as pd\nx = xr.open_dataset('a.nc')\nx = pd.read_csv('a.csv')\n",
            &["x"],
        );
        assert_eq!(got, vec![None]);
    }

    #[test]
    fn function_parameters_stay_unknown() {
        let file =
            parse_source("import xarray as xr\ndef f(ds):\n    return ds.values\n".to_string())
                .unwrap();
        // `ds` is a parameter: no binding anywhere, so rules keep their
        // pre-existing behaviour rather than silently going quiet.
        let root = file.tree.root_node();
        assert_eq!(file.bindings.lookup(root.id(), "ds"), None);
    }

    #[test]
    fn loop_variables_shadow_outer_bindings() {
        let file = parse_source(
            "import xarray as xr\nds = xr.open_dataset('a.nc')\nfor ds in items:\n    pass\n"
                .to_string(),
        )
        .unwrap();
        let root = file.tree.root_node();
        assert_eq!(file.bindings.lookup(root.id(), "ds"), None);
    }
}
