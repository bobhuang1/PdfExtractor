//! PPTX parser — an OPC package (ZIP + PresentationML).
//!
//! A presentation is read the way its own parts describe it: `ppt/presentation.xml`
//! names the slides, fixes their order and states the slide size; the
//! presentation's relationships name each slide's part; a slide's relationships
//! name the layout it was built on, the pictures it shows and the master above
//! the layout. What comes out is one [`Slide`] per slide — pages in the
//! document, as they are in Docling's own PPTX reading — holding the shapes in
//! the order a reader meets them: text bodies with their paragraphs and runs,
//! `graphicFrame` tables with their spans, pictures with the media part their
//! relationship points at — a shape filled with a picture is reported as that
//! picture, the fill being the only ink it has — and charts with the categories
//! and series their chart part has cached. A picture the slide states as its
//! background is not a shape: it is [`Slide::background`], read from
//! `p:cSld/p:bg/p:bgPr/a:blipFill` and covering the page.
//!
//! A slide also carries what the deck says *about* it: its speaker notes, from
//! the notes part its relationship names, and its comments, from the comments
//! part, with the authors named by the presentation's comment-authors part.
//! Both are the notes content layer in the document, not the slide's body.
//!
//! Legacy `.ppt` is a binary stream, not this format; it is not read here.

mod parser;

pub use parser::{
    chart_cell_text, ChartKind, ChartSeries, ChartShape, Comment, ListMarker, Paragraph,
    PictureShape, PptxDocument, Run, Shape, Slide, Table, TableCell, TableShape, TextShape,
};

#[derive(Debug, Default)]
pub struct PptxParser {}

impl PptxParser {
    pub fn new() -> Self {
        Self {}
    }
}
