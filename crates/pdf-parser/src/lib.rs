//! PDF parser - implementing from spec as per project plan.
//! Following hybrid approach: implement domain logic ourselves.

pub mod content;
pub mod encoding;
pub mod filters;
pub mod images;
pub mod lexer;
pub mod objects;
pub mod parser;

pub use content::{ContentParser, TextSpan};
pub use images::PageImage;
pub use parser::{ExtractedPage, PageLink, PdfDocument, PlacedImage};

#[derive(Debug, Default)]
pub struct PdfParser {}

impl PdfParser {
    pub fn new() -> Self {
        Self {}
    }
}
