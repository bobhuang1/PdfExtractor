//! ODP parser — an OpenDocument presentation (ODF 1.2/1.3).
//!
//! An `.odp` is a ZIP holding `content.xml`, `styles.xml` and the pictures a
//! slide places. `content.xml`'s `office:presentation` holds one `draw:page`
//! per slide, in presentation order. A page's `draw:frame` and
//! `draw:custom-shape` children are read as the shapes a reader meets: text
//! boxes (with paragraphs and nested lists), tables with their spans, and
//! pictures whose `draw:image` names a part in `Pictures/`.
//!
//! A slide is a page. The page rectangle comes from the `style:page-layout`
//! its master page names in `styles.xml`, in English Metric Units — the same
//! unit the PPTX reader reports — so a slide's box and its shapes are in one
//! coordinate system.
//!
//! A slide's `presentation:notes` child is its speaker notes; the notes frame
//! is read and comments written directly under the notes section are not. ODP
//! comments and animation are not read yet.

mod parser;

pub use parser::{
    OdpDocument, Paragraph, PictureShape, Shape, Slide, Table, TableCell, TableShape, TextShape,
};
