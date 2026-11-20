//! Delimited text (`.csv`, `.tsv`) parsing.
//!
//! Follows RFC 4180: fields separated by the delimiter, records by CRLF (or a
//! bare LF/CR), and a quoted field may contain the delimiter, a newline, or a
//! doubled `""` standing for one quote. The whole file is one table, so the
//! row/column structure the format exists to express is preserved.

use crate::{Block, TableGrid, TextDocument};
use pdfextractor_limits::{Guard, Limits, Result};

/// Parse delimited text with `delimiter`, typically `,` or `\t`.
pub fn parse(bytes: &[u8], delimiter: u8) -> TextDocument {
    parse_with_guard(bytes, delimiter, &Guard::unbudgeted())
        .expect("a guard with no budget refuses nothing")
}

/// Parse delimited text under `budget`: a workbook-sized grid counts one block
/// per cell against `max_flow_blocks`.
pub fn parse_with_limits(bytes: &[u8], delimiter: u8, budget: &Limits) -> Result<TextDocument> {
    parse_with_guard(bytes, delimiter, &budget.guard())
}

fn parse_with_guard(bytes: &[u8], delimiter: u8, guard: &Guard<'_>) -> Result<TextDocument> {
    guard.check_input(bytes.len())?;
    let rows = records_with_guard(bytes, delimiter, guard)?;
    let mut doc = TextDocument::default();
    if !rows.is_empty() {
        // A delimited file says nothing about merges: every field is one slot.
        doc.items.push(Block::Table {
            grid: TableGrid::from_rows(&rows),
        });
    }
    guard.check_flow_blocks(crate::flow_units(&doc.items))?;
    Ok(doc)
}

/// Split delimited bytes into records of fields.
pub fn records(bytes: &[u8], delimiter: u8) -> Vec<Vec<String>> {
    records_with_guard(bytes, delimiter, &Guard::unbudgeted())
        .expect("a guard with no budget refuses nothing")
}

fn records_with_guard(bytes: &[u8], delimiter: u8, guard: &Guard<'_>) -> Result<Vec<Vec<String>>> {
    let mut rows = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field: Vec<u8> = Vec::new();
    let mut quoted = false;
    let mut i = 0usize;

    while i < bytes.len() {
        if i.is_multiple_of(65536) {
            guard.check("reading a delimited record")?;
        }
        let b = bytes[i];
        if quoted {
            if b == b'"' {
                if bytes.get(i + 1) == Some(&b'"') {
                    field.push(b'"');
                    i += 2;
                    continue;
                }
                quoted = false;
                i += 1;
            } else {
                field.push(b);
                i += 1;
            }
            continue;
        }
        match b {
            b'"' if field.is_empty() => {
                quoted = true;
                i += 1;
            }
            b'"' => {
                // A stray quote mid-field is literal.
                field.push(b);
                i += 1;
            }
            _ if b == delimiter => {
                row.push(field_string(&field));
                field.clear();
                i += 1;
            }
            b'\r' => {
                row.push(field_string(&field));
                field.clear();
                rows.push(std::mem::take(&mut row));
                i += if bytes.get(i + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
            }
            b'\n' => {
                row.push(field_string(&field));
                field.clear();
                rows.push(std::mem::take(&mut row));
                i += 1;
            }
            _ => {
                field.push(b);
                i += 1;
            }
        }
    }

    // A final field/record with no trailing newline.
    if !field.is_empty() || !row.is_empty() {
        row.push(field_string(&field));
        rows.push(row);
    }
    Ok(rows)
}

fn field_string(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| crate::cp1252(b)).collect(),
    }
}

/// Parse comma-separated values.
pub fn parse_csv(bytes: &[u8]) -> TextDocument {
    parse(bytes, b',')
}

/// Parse comma-separated values under `budget`.
pub fn parse_csv_with_limits(bytes: &[u8], budget: &Limits) -> Result<TextDocument> {
    parse_with_limits(bytes, b',', budget)
}

/// Parse tab-separated values.
pub fn parse_tsv(bytes: &[u8]) -> TextDocument {
    parse(bytes, b'\t')
}

/// Parse tab-separated values under `budget`.
pub fn parse_tsv_with_limits(bytes: &[u8], budget: &Limits) -> Result<TextDocument> {
    parse_with_limits(bytes, b'\t', budget)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A grid counts one block per cell, so a caller that lowers the block
    /// budget refuses a file whose grid is wider than it allows.
    #[test]
    fn a_custom_page_budget_is_enforced() {
        let budget = Limits {
            max_flow_blocks: 3,
            ..Limits::default()
        };
        let error =
            parse_with_limits(b"a,b\n1,2\n", b',', &budget).expect_err("the file has four cells");
        assert!(error.to_string().contains("4 blocks"), "{error}");
        assert!(error.to_string().contains("limit is 3"), "{error}");
    }

    /// The walk checks the clock as it goes, so a spent deadline refuses a file
    /// however small it is.
    #[test]
    fn a_spent_deadline_is_enforced() {
        let budget = Limits {
            timeout: Duration::ZERO,
            ..Limits::default()
        };
        let error = parse_with_limits(b"a,b\n", b',', &budget).expect_err("the deadline is spent");
        assert!(error.to_string().contains("timed out"), "{error}");
    }

    #[test]
    fn simple_rows_and_columns() {
        let rows = records(b"a,b\n1,2\n", b',');
        assert_eq!(
            rows,
            vec![
                vec!["a".to_string(), "b".to_string()],
                vec!["1".to_string(), "2".to_string()],
            ]
        );
    }

    #[test]
    fn quoted_fields_may_hold_delimiters_and_newlines() {
        let rows = records(b"\"a,b\",\"line1\nline2\"\n", b',');
        assert_eq!(
            rows,
            vec![vec!["a,b".to_string(), "line1\nline2".to_string()]]
        );
    }

    #[test]
    fn doubled_quotes_are_one_quote() {
        let rows = records(b"\"say \"\"hi\"\"\"\n", b',');
        assert_eq!(rows, vec![vec!["say \"hi\"".to_string()]]);
    }

    #[test]
    fn crlf_does_not_leave_an_empty_row() {
        let rows = records(b"a,b\r\n1,2\r\n", b',');
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn tsv_uses_tabs() {
        let rows = records(b"a\tb\n", b'\t');
        assert_eq!(rows, vec![vec!["a".to_string(), "b".to_string()]]);
    }
}
