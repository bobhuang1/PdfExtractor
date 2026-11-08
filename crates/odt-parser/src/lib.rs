//! ODT parser — an OpenDocument text document (ODF 1.2/1.3).
//!
//! A `.odt` is a ZIP holding `content.xml`, `styles.xml` and the pictures the
//! document places. The body of `content.xml` is read in document order and
//! reported as the blocks the document has: paragraphs and headings, list
//! items, tables (with their spans) and pictures. `styles.xml` is read for the
//! meaning of a paragraph's style — the outline level that makes a paragraph a
//! heading, where a producer leaves the level off the element.

mod parser;

pub use parser::{OdtCell, OdtDocument, OdtItem, OdtPicture, OdtTable};
