//! DOCX parser - OPC package (ZIP + XML)
//! Using zip and quick-xml for infrastructure; implement DOCX structure parsing ourselves.

mod drawing;
pub mod parser;

pub use parser::{DocxDocument, DocxItem};

#[derive(Debug, Default)]
pub struct DocxParser {}

impl DocxParser {
    pub fn new() -> Self {
        Self {}
    }
}
