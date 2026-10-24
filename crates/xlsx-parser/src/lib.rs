//! XLSX parser — an OPC package (ZIP + SpreadsheetML).
//!
//! A workbook is read the way its format describes it: `xl/workbook.xml` names
//! the sheets and fixes their order, `xl/sharedStrings.xml` holds the strings a
//! cell points at, `xl/styles.xml` says which numbers are dates, and each
//! worksheet part holds that sheet's cells and merged ranges. What comes out is
//! one [`Sheet`] per worksheet, each holding the [`SheetTable`]s its cells form
//! — the shape Docling's own XLSX backend produces.

mod parser;

pub use parser::{Sheet, SheetCell, SheetPicture, SheetTable, XlsxDocument};

#[derive(Debug, Default)]
pub struct XlsxParser {}

impl XlsxParser {
    pub fn new() -> Self {
        Self {}
    }
}
