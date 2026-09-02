//! Jupyter notebook (`.ipynb`) support.
//!
//! Extracts code cells from a notebook JSON file, strips IPython magic commands
//! (lines starting with `%` or `!`) by replacing them with blank lines so that
//! intra-cell line numbers are preserved, then passes each cell through the
//! normal `parse_source` → `rules::run_all` pipeline.
//!
//! Import context is accumulated across all cells so that `import xarray` in
//! cell 1 correctly gates xarray rules in cell 5.

use anyhow::Result;
use serde_json::Value;

use crate::parser::{self, ImportContext, ParsedFile};

/// A single code cell extracted from a Jupyter notebook, ready for linting.
pub struct NotebookCell {
    /// 1-based index counting only *code* cells (markdown/raw cells are skipped).
    pub index: usize,
    /// Human-readable label for parse errors, e.g. `analysis.ipynb:cell[3]`.
    /// Diagnostics keep the real notebook path and record the cell separately.
    pub label: String,
    /// Cell source with magic lines blanked out (blank lines preserve the
    /// original per-cell line numbers in diagnostics).
    pub source: String,
    /// Parsed AST + import context for this cell.
    pub parsed: ParsedFile,
}

/// Parse a `.ipynb` file and return one [`NotebookCell`] per code cell.
///
/// Magic command lines (`%...` and `!...`) are replaced with blank lines before
/// parsing so that tree-sitter sees valid Python and line numbers stay correct.
///
/// After all cells are parsed, their [`ImportContext`]s are merged so that an
/// `import xarray` in cell 1 gates xarray rules in cell 5.
pub fn parse_notebook(path: &str) -> Result<Vec<NotebookCell>> {
    let bytes = std::fs::read(path).map_err(|e| anyhow::anyhow!("cannot read {path}: {e}"))?;
    let nb: Value = serde_json::from_slice(&bytes)
        .map_err(|e| anyhow::anyhow!("cannot parse notebook JSON {path}: {e}"))?;

    let cells = nb["cells"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("notebook {path}: missing top-level 'cells' array"))?;

    let mut notebook_cells: Vec<NotebookCell> = Vec::new();
    let mut code_cell_index = 0usize;

    for cell in cells {
        // Only lint code cells — skip markdown and raw cells.
        if cell["cell_type"].as_str() != Some("code") {
            continue;
        }
        code_cell_index += 1;

        let raw_source = extract_cell_source(&cell["source"]);
        let cleaned = strip_magics(&raw_source);
        let label = format!("{}:cell[{}]", path, code_cell_index);

        match parser::parse_source(cleaned.clone()) {
            Ok(parsed) => {
                notebook_cells.push(NotebookCell {
                    index: code_cell_index,
                    label,
                    source: cleaned,
                    parsed,
                });
            }
            Err(e) => {
                // A cell that fails to parse (e.g. syntax error) is skipped
                // with a warning rather than aborting the whole notebook.
                eprintln!("xray: could not parse {label}: {e}");
            }
        }
    }

    // Merge import contexts so that `import xarray` in cell 1 enables xarray
    // rules in cell 5.  We overwrite every cell's context with the union.
    if notebook_cells.len() > 1 {
        let merged = merge_imports(&notebook_cells);
        for cell in &mut notebook_cells {
            cell.parsed.imports = merged.clone();
        }
    }

    Ok(notebook_cells)
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Build an `ImportContext` that is the logical OR of all cells' import flags.
fn merge_imports(cells: &[NotebookCell]) -> ImportContext {
    let mut merged = ImportContext::default();
    for cell in cells {
        let i = &cell.parsed.imports;
        merged.xarray |= i.xarray;
        merged.dask |= i.dask;
        merged.numpy |= i.numpy;
        merged.pandas |= i.pandas;
        merged.netcdf4 |= i.netcdf4;
        merged.zarr |= i.zarr;
        merged.h5py |= i.h5py;
    }
    merged
}

/// Extract the source string from a cell's `"source"` field, which can be
/// either a plain string (some nbformat versions) or an array of line strings
/// (standard nbformat 4).
fn extract_cell_source(val: &Value) -> String {
    match val {
        Value::String(s) => s.clone(),
        Value::Array(lines) => lines
            .iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Replace IPython magic lines (`%magic` / `!shell`) with blank lines.
///
/// We replace rather than remove so that tree-sitter row numbers stay aligned
/// with the original cell line numbers — diagnostics report the right line.
fn strip_magics(source: &str) -> String {
    source
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            // `%` — IPython line magic / cell magic (`%%`)
            // `!` — shell escape
            if trimmed.starts_with('%') || trimmed.starts_with('!') {
                ""
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_magics_preserves_line_numbers() {
        let src = "import numpy as np\n%matplotlib inline\nx = np.zeros(10)\n!ls -la\ny = x + 1\n";
        let result = strip_magics(src);
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0], "import numpy as np");
        assert_eq!(lines[1], ""); // %matplotlib replaced with blank
        assert_eq!(lines[2], "x = np.zeros(10)");
        assert_eq!(lines[3], ""); // !ls replaced with blank
        assert_eq!(lines[4], "y = x + 1");
    }

    #[test]
    fn strip_magics_leaves_normal_code_unchanged() {
        let src = "x = 1\ny = 2\n";
        assert_eq!(strip_magics(src), "x = 1\ny = 2");
    }

    #[test]
    fn extract_cell_source_handles_array() {
        let val: Value = serde_json::json!(["import numpy\n", "x = 1\n"]);
        assert_eq!(extract_cell_source(&val), "import numpy\nx = 1\n");
    }

    #[test]
    fn extract_cell_source_handles_string() {
        let val: Value = serde_json::json!("import numpy\nx = 1\n");
        assert_eq!(extract_cell_source(&val), "import numpy\nx = 1\n");
    }
}
