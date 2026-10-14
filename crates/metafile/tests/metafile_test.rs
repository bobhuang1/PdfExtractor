//! Tests for the metafile players: a hand-built WMF that checks the coordinate
//! conventions, and the real drawings Word wrote in the corpus.

use pdfextractor_metafile::{self as metafile, Options, Raster};
use std::io::Read;

// ---------------------------------------------------------------- hand-built

/// A placeable WMF header: the key, a bounding box in metafile units, and
/// `units_per_inch`.
fn placeable(left: i16, top: i16, right: i16, bottom: i16, units_per_inch: u16) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0x9ac6_cdd7u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // handle, unused
    for value in [left, top, right, bottom] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&units_per_inch.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // reserved
    out.extend_from_slice(&0u16.to_le_bytes()); // checksum, unused
    out
}

/// The 18-byte `METAHEADER` that follows a placeable header.
fn meta_header() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&1u16.to_le_bytes()); // memory metafile
    out.extend_from_slice(&9u16.to_le_bytes()); // nine words
    out.extend_from_slice(&0x0300u16.to_le_bytes()); // version
    out.extend_from_slice(&0u32.to_le_bytes()); // size in words, unused
    out.extend_from_slice(&0u16.to_le_bytes()); // no objects
    out.extend_from_slice(&0u32.to_le_bytes()); // max record, unused
    out.extend_from_slice(&0u16.to_le_bytes()); // no parameters
    out
}

/// A record: its size in words, its function, then its parameters.
fn record(function: u16, params: &[u8]) -> Vec<u8> {
    let bytes = 6 + params.len();
    assert_eq!(bytes % 2, 0, "a record is a whole number of words");
    let mut out = Vec::new();
    out.extend_from_slice(&((bytes / 2) as u32).to_le_bytes());
    out.extend_from_slice(&function.to_le_bytes());
    out.extend_from_slice(params);
    out
}

/// `SETWINDOWORG`/`SETWINDOWEXT` write the vertical component first.
fn window_record(function: u16, y: i16, x: i16) -> Vec<u8> {
    let mut params = Vec::new();
    params.extend_from_slice(&y.to_le_bytes());
    params.extend_from_slice(&x.to_le_bytes());
    record(function, &params)
}

/// A `POLYGON` record: a count, then `POINT` structures.
fn polygon(points: &[(i16, i16)]) -> Vec<u8> {
    let mut params = Vec::new();
    params.extend_from_slice(&(points.len() as u16).to_le_bytes());
    for (x, y) in points {
        params.extend_from_slice(&x.to_le_bytes());
        params.extend_from_slice(&y.to_le_bytes());
    }
    record(0x0324, &params)
}

/// A WMF whose window is `100 x 10` units and which fills a wide, flat bar in
/// its top fifth.
fn bar_drawing() -> Vec<u8> {
    let mut data = placeable(0, 0, 1000, 1000, 1000);
    data.extend_from_slice(&meta_header());
    data.extend_from_slice(&window_record(0x020b, 0, 0));
    data.extend_from_slice(&window_record(0x020c, 10, 100));
    data.extend_from_slice(&polygon(&[(10, 0), (90, 0), (90, 2), (10, 2)]));
    data.extend_from_slice(&record(0x0000, &[0, 0]));
    data
}

/// The ink of a raster: the rows and columns it touches, and how much of it
/// there is.
fn ink_span(raster: &Raster) -> (usize, usize, usize, usize, usize) {
    let (mut left, mut top, mut right, mut bottom, mut count) = (
        raster.width as usize,
        raster.height as usize,
        0usize,
        0usize,
        0usize,
    );
    for (index, px) in raster.pixels.chunks_exact(4).enumerate() {
        if px[0] == 255 && px[1] == 255 && px[2] == 255 {
            continue;
        }
        let (x, y) = (index % raster.width as usize, index / raster.width as usize);
        left = left.min(x);
        top = top.min(y);
        right = right.max(x);
        bottom = bottom.max(y);
        count += 1;
    }
    (left, top, right, bottom, count)
}

/// The points of a `POLYGON` are `POINT` structures, which write x before y —
/// unlike `SETWINDOWORG`, whose two words are y then x. Reading them the other
/// way round turns every WMF polygon into its own transpose, and CorelDRAW's
/// drawings come out rotated and half outside the raster.
#[test]
fn polygon_points_are_x_then_y() {
    let raster = metafile::rasterize(&bar_drawing(), Options::default().at_size(100.0, 10.0))
        .expect("a bar drawing");
    assert_eq!(
        (raster.width, raster.height),
        (200, 20),
        "the canvas is the caller's size"
    );
    let (left, top, right, bottom, count) = ink_span(&raster);
    assert!(count > 0, "nothing was drawn");
    // A bar across the width, in the top fifth: the other reading gives a
    // sliver of a few columns, mostly below the raster.
    let width = right - left + 1;
    let height = bottom - top + 1;
    assert!(
        width * 10 >= raster.width as usize * 7,
        "the bar spans {width} of {} columns: it was transposed",
        raster.width
    );
    assert!(
        height * 10 <= raster.height as usize * 3,
        "the bar is {height} of {} rows tall: it was transposed",
        raster.height
    );
    assert!(
        top < raster.height as usize / 4,
        "the bar is not at the top: row {top}"
    );
}

/// Bytes that are not a metafile are refused, and so is a metafile that draws
/// nothing: a caller must not be handed a picture of nothing.
#[test]
fn refuses_what_it_cannot_draw() {
    assert!(metafile::rasterize(b"not a metafile", Options::default()).is_err());
    let mut empty = placeable(0, 0, 1000, 1000, 1000);
    empty.extend_from_slice(&meta_header());
    empty.extend_from_slice(&record(0x0000, &[0, 0]));
    assert!(
        metafile::rasterize(&empty, Options::default()).is_err(),
        "a metafile that draws nothing is not a picture"
    );
    // A drawing with a size but a truncated record stream is malformed.
    let mut truncated = bar_drawing();
    truncated.truncate(truncated.len() - 12);
    assert!(metafile::rasterize(&truncated, Options::default()).is_err());
}

// ------------------------------------------------------------------- corpus

/// The EMF and WMF drawings Word wrote into `word/media/`.
///
/// `mathtype.docx` is here for the shape of drawing a document can hide: the
/// EMF is a MathType equation's preview, reached through a `w:object` rather
/// than a `w:drawing`.
fn corpus_drawings(docx: &str) -> Vec<(String, Vec<u8>)> {
    let path = std::path::PathBuf::from("../../test-data/real").join(docx);
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("missing {}: {e}", path.display()));
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("a DOCX is a ZIP");
    let mut out = Vec::new();
    for index in 0..zip.len() {
        let mut file = zip.by_index(index).unwrap();
        let name = file.name().to_string();
        let drawing =
            name.starts_with("word/media/") && (name.ends_with(".emf") || name.ends_with(".wmf"));
        if !drawing {
            continue;
        }
        let mut data = Vec::new();
        file.read_to_end(&mut data).unwrap();
        out.push((name, data));
    }
    out
}

/// Rasterise a drawing at the size its own frame declares.
fn rasterize_drawing(data: &[u8]) -> Raster {
    metafile::rasterize(data, Options::default()).expect("a corpus drawing")
}

/// Every drawing in the corpus plays, at a size of its own frame's aspect, and
/// paints something.
#[test]
fn corpus_drawings_rasterise() {
    let mut seen = 0;
    for docx in ["VariousPictures.docx", "drawing.docx", "mathtype.docx"] {
        for (name, data) in corpus_drawings(docx) {
            let raster = rasterize_drawing(&data);
            let (_, _, _, _, count) = ink_span(&raster);
            assert!(count > 0, "{docx} {name}: nothing was drawn");
            assert!(
                raster.unsupported.is_empty(),
                "{docx} {name}: unsupported records {:?}",
                raster.unsupported
            );
            assert!(raster.to_png().is_ok(), "{docx} {name}: cannot be encoded");
            seen += 1;
        }
    }
    assert_eq!(seen, 5, "the corpus holds five metafile drawings");
}

/// A drawing is meant to fill the frame it declares, so its ink has to reach
/// across that frame. A cropped or transposed playback does not, and this is
/// what caught the `POINT` order: CorelDRAW's Santa came out rotated and with a
/// quarter of its ink outside the raster.
#[test]
fn drawings_fill_the_frame_they_declare() {
    for (name, data) in corpus_drawings("VariousPictures.docx") {
        let raster = rasterize_drawing(&data);
        let (left, top, right, bottom, count) = ink_span(&raster);
        let width = (right - left + 1) as f64 / raster.width as f64;
        let height = (bottom - top + 1) as f64 / raster.height as f64;
        assert!(
            width > 0.9 && height > 0.9,
            "{name}: ink covers {width:.2} x {height:.2} of the frame"
        );
        // A picture of a drawing is not a handful of stray pixels.
        let coverage = count as f64 / (raster.width as f64 * raster.height as f64);
        assert!(
            coverage > 0.05,
            "{name}: only {coverage:.3} of the frame is inked"
        );
    }
}

/// The raster of a corpus drawing really is a PNG a decoder can read, at the
/// pixel size the hardware says.
#[test]
fn corpus_rasters_are_readable_pngs() {
    for (name, data) in corpus_drawings("VariousPictures.docx") {
        let raster = rasterize_drawing(&data);
        let png = raster.to_png().expect("encoded");
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        let decoded = image::load_from_memory(&png).expect("a PNG a decoder can read");
        assert_eq!(
            (decoded.width(), decoded.height()),
            (raster.width, raster.height),
            "{name}: the PNG is not the raster it claims"
        );
    }
}
