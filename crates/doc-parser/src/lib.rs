//! Legacy Word (`.doc`) parser.
//!
//! Implements the OLE compound-file container and the Word binary text model
//! from the ground up. See [`word`] for what is and is not recovered, and
//! [`cfb`] for the container reader.

pub mod cfb;
pub mod word;

pub use cfb::CompoundFile;
pub use word::{DocDocument, DocItem};

#[derive(Debug, Default)]
pub struct DocParser {}

impl DocParser {
    pub fn new() -> Self {
        Self {}
    }
}
