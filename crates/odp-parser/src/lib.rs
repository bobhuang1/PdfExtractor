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
//! A slide's `presentation:notes` child is its speaker notes; only the notes
//! frame is a speaker note. A slide's comments are `officeooo:annotation`
//! children of its `draw:page`, read with their author, date and text.
//!
//! A `draw:frame` can hold an embedded chart object rather than a picture. Its
//! `draw:object` names an `Object N` directory whose `content.xml` is read as
//! the chart's title, family, categories and series, using the chart's own
//! `local-table` for the plotted cells; the frame supplies the box. Master-page
//! background shapes and animation are not read yet.

mod parser;

pub use parser::{
    Comment, OdpDocument, Paragraph, PictureShape, Shape, Slide, Table, TableCell, TableShape,
    TextShape,
};
