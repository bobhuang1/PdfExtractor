//! Turning a vector drawing in a DOCX into a picture.
//!
//! Word does not only put bitmaps in `word/media/`. A vector drawing, and the
//! preview of an embedded object such as a MathType equation, is stored as an
//! Enhanced Metafile (`image/emf`) or a Windows Metafile (`image/wmf`) — a list
//! of GDI calls, not a grid of pixels.
//!
//! Neither media type can be named in a Docling `image` reference, and neither
//! has a pixel size or a resolution, so such a picture used to be reported as a
//! box with no image: the drawing was invisible to everything downstream. The
//! records are played instead (see `pdfextractor_metafile`), which is how these
//! files get pixels and a name Docling accepts.
//!
//! This is conversion at the boundary, not invention: the raster is produced at
//! the size the document asks the drawing to be drawn at, at the same 144 dpi
//! Docling's own DOCX backend renders these drawings at.

use pdfextractor_metafile as metafile;

/// Whether a media type is a drawing to be played rather than encoded pixels.
pub(crate) fn is_vector_drawing(mime: &str) -> bool {
    matches!(mime, "image/emf" | "image/wmf")
}

/// Play a metafile into a PNG, at the size the document asks it to be drawn.
///
/// `None` when there is no picture to be had from these bytes: the drawing is
/// malformed, too large to rasterise, or paints nothing at all. The caller then
/// keeps the payload exactly as it was, so a drawing we cannot play is still
/// reported with its box and no image rather than as a blank picture.
pub(crate) fn rasterize_to_png(
    data: &[u8],
    width_pt: Option<f64>,
    height_pt: Option<f64>,
) -> Option<Vec<u8>> {
    let mut options = metafile::Options::default();
    if let (Some(width), Some(height)) = (width_pt, height_pt) {
        options = options.at_size(width, height);
    }
    let raster = match metafile::rasterize(data, options) {
        Ok(raster) => raster,
        Err(err) => {
            tracing::debug!("leaving a drawing unrasterised: {err}");
            return None;
        }
    };
    // A drawing that put no ink down is not a picture of anything, and a white
    // rectangle is not worth naming as one.
    if raster.inked_pixels() == 0 {
        tracing::debug!("a drawing rasterised to an empty picture");
        return None;
    }
    match raster.to_png() {
        Ok(png) => Some(png),
        Err(err) => {
            tracing::debug!("cannot encode a rasterised drawing: {err}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bytes that are not a metafile are refused rather than turned into a
    /// picture of nothing.
    #[test]
    fn refuses_bytes_that_are_not_a_drawing() {
        assert!(rasterize_to_png(b"not a drawing at all", Some(100.0), Some(50.0)).is_none());
        assert!(rasterize_to_png(&[], None, None).is_none());
        // A PNG is a picture already; it is not a metafile to play.
        assert!(rasterize_to_png(
            &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a],
            None,
            None
        )
        .is_none());
    }

    /// A truncated metafile is not guessed at.
    #[test]
    fn refuses_a_truncated_drawing() {
        // The EMF signature, then nothing: a file that claims to be a metafile
        // but has no records must not produce a picture.
        let mut header = vec![1u8, 0, 0, 0];
        header.extend_from_slice(&[0u8; 84]);
        assert!(rasterize_to_png(&header, None, None).is_none());
    }

    #[test]
    fn only_metafiles_are_drawings() {
        assert!(is_vector_drawing("image/emf"));
        assert!(is_vector_drawing("image/wmf"));
        assert!(!is_vector_drawing("image/png"));
        assert!(!is_vector_drawing("image/svg+xml"));
    }
}
