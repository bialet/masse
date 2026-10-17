//! Read-only spreadsheet access (`.xls`, `.xlsx`, `.xlsb`, `.ods`).
//!
//! Reading is delegated to `calamine`, which is pure Rust and needs no external
//! binary. The workbook is parsed on a blocking thread because calamine does
//! synchronous I/O.

use async_trait::async_trait;
use calamine::{open_workbook_auto, Data, Range, Reader};
use serde_json::Value as Json;

use crate::tool::{opt_str, opt_u64, req_str, schema_object, Tool, ToolContext, ToolResult};

const DEFAULT_MAX_ROWS: usize = 200;
const DEFAULT_MAX_COLS: usize = 50;
const MAX_OUTPUT_BYTES: usize = 256 * 1024;

// ---------------------------------------------------------------------------
// read_spreadsheet
// ---------------------------------------------------------------------------

pub struct ReadSpreadsheet;

#[async_trait]
impl Tool for ReadSpreadsheet {
    fn name(&self) -> &str {
        "read_spreadsheet"
    }

    fn description(&self) -> &str {
        "Read a spreadsheet (.xls, .xlsx, .xlsb, .ods) inside the project. Returns the sheet names and cell values as tab-separated rows. Use `sheet` to pick one; row/column limits keep large sheets readable."
    }

    fn schema(&self) -> Json {
        schema_object(
            &[
                (
                    "path",
                    "string",
                    "Spreadsheet path relative to the project root (or absolute inside it).",
                ),
                (
                    "sheet",
                    "string",
                    "Sheet name to read. Default: every sheet.",
                ),
                (
                    "max_rows",
                    "integer",
                    "Maximum data rows per sheet. Default: 200.",
                ),
                (
                    "max_cols",
                    "integer",
                    "Maximum columns per row. Default: 50.",
                ),
            ],
            &["path"],
        )
    }

    async fn call(&self, ctx: &ToolContext, args: Json) -> ToolResult {
        let path = match req_str(&args, "path").and_then(|p| ctx.resolve_existing(&p)) {
            Ok(p) => p,
            Err(e) => return ToolResult::err(e),
        };
        let display = ctx.display_path(&path);
        let sheet = opt_str(&args, "sheet");
        let max_rows = opt_u64(&args, "max_rows")
            .map(|v| v as usize)
            .unwrap_or(DEFAULT_MAX_ROWS)
            .max(1);
        let max_cols = opt_u64(&args, "max_cols")
            .map(|v| v as usize)
            .unwrap_or(DEFAULT_MAX_COLS)
            .max(1);

        let parsed = tokio::task::spawn_blocking(move || {
            read_workbook(&path, sheet.as_deref(), max_rows, max_cols)
        })
        .await;

        match parsed {
            Ok(Ok(body)) => ToolResult::ok(format!("{display}\n{body}")),
            Ok(Err(e)) => ToolResult::err(e),
            Err(e) => ToolResult::err(format!("spreadsheet reader failed: {e}")),
        }
    }
}

fn read_workbook(
    path: &std::path::Path,
    requested: Option<&str>,
    max_rows: usize,
    max_cols: usize,
) -> Result<String, String> {
    let mut workbook =
        open_workbook_auto(path).map_err(|e| format!("cannot open spreadsheet: {e}"))?;
    let names = workbook.sheet_names();
    if names.is_empty() {
        return Err("spreadsheet has no sheets".to_string());
    }

    let selected: Vec<String> = match requested {
        Some(want) => {
            let name = names
                .iter()
                .find(|n| n.eq_ignore_ascii_case(want))
                .ok_or_else(|| {
                    format!("sheet '{want}' not found; available: {}", names.join(", "))
                })?;
            vec![name.clone()]
        }
        None => names.clone(),
    };

    let mut out = format!("sheets: {}\n", names.join(", "));
    for name in selected {
        let range = workbook
            .worksheet_range(&name)
            .map_err(|e| format!("cannot read sheet '{name}': {e}"))?;
        if let Some(reason) = render_sheet(&mut out, &name, &range, max_rows, max_cols) {
            out.push_str(reason);
            break;
        }
    }
    Ok(out)
}

/// Append one sheet to `out`. Returns `Some(reason)` when the shared output cap
/// was hit, so the caller stops rendering further sheets.
fn render_sheet(
    out: &mut String,
    name: &str,
    range: &Range<Data>,
    max_rows: usize,
    max_cols: usize,
) -> Option<&'static str> {
    let (rows, cols) = range.get_size();
    out.push_str(&format!("\n## {name} ({rows} rows x {cols} cols)\n"));

    let mut written = 0;
    for (index, row) in range.rows().enumerate() {
        if index >= max_rows {
            break;
        }
        let line = row
            .iter()
            .take(max_cols)
            .map(cell_text)
            .collect::<Vec<_>>()
            .join("\t");
        if out.len() + line.len() + 1 > MAX_OUTPUT_BYTES {
            return Some("[masse] output truncated\n");
        }
        out.push_str(&line);
        out.push('\n');
        written += 1;
    }

    if written < rows {
        out.push_str(&format!("[masse] showing {written} of {rows} rows\n"));
    }
    if cols > max_cols {
        out.push_str(&format!("[masse] showing {max_cols} of {cols} columns\n"));
    }
    None
}

fn cell_text(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_cells_render_as_empty_text() {
        assert_eq!(cell_text(&Data::Empty), "");
    }

    #[test]
    fn values_render_without_quotes() {
        assert_eq!(cell_text(&Data::String("hola".into())), "hola");
        assert_eq!(cell_text(&Data::Int(42)), "42");
        assert_eq!(cell_text(&Data::Bool(true)), "true");
    }

    #[test]
    fn a_sheet_respects_the_row_and_column_caps() {
        let mut range = Range::new((0, 0), (2, 2));
        range.set_value((0, 0), Data::String("a".into()));
        range.set_value((0, 1), Data::String("b".into()));
        range.set_value((1, 0), Data::String("c".into()));

        let mut out = String::new();
        assert!(render_sheet(&mut out, "S1", &range, 1, 1).is_none());
        assert!(out.contains("a"));
        assert!(out.contains("showing 1 of 3 rows"));
        assert!(out.contains("showing 1 of 3 columns"));
        assert!(!out.contains('b'));
    }
}
