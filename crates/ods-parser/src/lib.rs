//! ODS parser — an OpenDocument spreadsheet (ODF 1.2/1.3).
//!
//! A `.ods` is a ZIP holding `content.xml` and the parts the workbook's styles
//! name. The body of `content.xml` is an `office:spreadsheet` whose every
//! `table:table` is a worksheet: its rows are `table:table-row`, its cells
//! `table:table-cell`, and a value is the text the cell displays (or the typed
//! value a producer writes instead). Each sheet is read into the workbook
//! model the XLSX reader produces, so a workbook is a workbook however it was
//! saved: a sheet is a page, a sheet group holds its tables, and the command
//! line's page range names sheets. A cell's `draw:frame` is read too: a
//! `draw:image` is a picture and a `draw:object` is an embedded chart, each
//! placed in the cell that anchors its frame.

mod parser;

pub use parser::OdsDocument;
