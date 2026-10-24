//! Painting a DrawingML shape into a picture.
//!
//! A shape in an office package has no image part behind it: the application
//! paints it from the markup. Word states one as a `wps:wsp` inside
//! `w:drawing`; PowerPoint states one as a `p:sp` inside a slide's shape tree —
//! the two allow the same DrawingML body (`a:xfrm`, `a:prstGeom`/`a:custGeom`,
//! `a:solidFill`, `a:ln`) and the same style references (`a:fillRef`,
//! `a:lnRef`), so one painter reads both. A reader that only looked for an
//! image relationship left such a shape out of the document altogether —
//! invisible where a page shows a box, an ellipse, an arrow or a star.
//!
//! The geometry becomes a polygon in the drawing's points, its fill and line
//! become ops, and [`paint`] rasterises those at 144 dpi. Each shape is placed
//! by its own `a:xfrm`, mirrored and turned as the transform says, and a group
//! (`wps:wgp`, `wpc:wpc`, `p:grpSp`) maps its children's coordinate space onto
//! its frame.
//!
//! A shape is shaded as the file states it. A `a:gradFill` is a ramp across the
//! shape — its stops with their own opacity, along an `a:lin` angle (taken in the
//! shape's proportions, or at its true angle when `scaled="0"`) or out from the
//! middle for an `a:path`. An `a:outerShdw` is the shape's silhouette, moved and
//! blurred, laid down under the ink. And a line's `a:headEnd`/`a:tailEnd` is a
//! triangle, stealth, diamond, oval or open head at that end, sized from the
//! line's own width, with the stroke stopped at its base.
//!
//! A fill can be a picture. An `a:blipFill` names its media the way a picture
//! does, and the fill is part of the shape: the picture is drawn through the
//! shape's own outline — a clip is what makes an ellipse's fill elliptical, and
//! not a picture in a box — stretched across its frame, cropped as `a:srcRect`
//! says and turned with the shape, under its line. The pixels come from the
//! caller, because only the reader that holds the package can resolve a
//! relationship id: [`Media`] is that lookup, and [`NoMedia`] the painter's
//! answer for a drawing with no package behind it.
//!
//! What is not drawn is the shape's text: the words in a `wps:txbx` or a
//! `p:txBody` are the document's own text, reported where they stand. That is
//! also why the text subtree is never descended into — a picture inside a text
//! box is placed by its own drawing, not painted again into its container.
//!
//! Colours are the file's, resolved through the package's theme: a
//! `a:schemeClr val="accent1"` is the accent the theme declares, its `shade`
//! and `tint` applied, because a shape's fill usually comes from its `style`'s
//! `a:fillRef` and the theme rather than from a literal `a:srgbClr`. Without
//! that, a real shape would paint black or not at all.
//!
//! Not drawn: patterns, 3-D, the effects other than an outer shadow (a glow, a
//! reflection, a bevel) and any preset outline not in
//! [`shape::preset_outline`]. A shape that puts no ink down is no picture rather
//! than a blank one.

pub mod paint;

mod shape;

pub use shape::{
    paints_a_picture_fill, render, render_node, render_node_with, render_with, Media, NoMedia,
    Picture, Theme,
};
