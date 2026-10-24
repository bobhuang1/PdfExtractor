//! The image reader against real encodings of the same scan.
//!
//! `test-data/make_scan_images.py` writes one 620 x 350 invoice through seven
//! encoders. Four of them declare 150 dpi, each in its own header field; WebP
//! has no resolution field, the JPEG 2000 fixture states none, and a GIF has
//! none to state, so their pages are one pixel per point. Two more files are the
//! same JPEG 2000: one with the capture resolution box a scanner writes, whose
//! page is the 150 dpi that box declares, and one as a bare codestream —
//! `.j2c`, the drawing without its JP2 container, whose `SIZ` marker states the
//! grid and which declares no density at all. Two more spell the same pair with
//! the other names JPEG 2000 files carry — `.jpc` for the codestream, `.jpf` for
//! the container — and the pair is deliberately crossed: the extension is only a
//! name, and which header to read is decided by the first bytes. The reader is
//! held to the page those headers describe, not to the pixels and not to the
//! name.
//!
//! A GIF is a page by its screen: the scan is a still image, so what the reader
//! takes from it is the one frame it holds, and the CLI's OCR test reads the same
//! words out of that frame as out of the WebP and the JP2. (An animation is
//! several frames, and the CLI writes a page for each — see
//! `gif_frames`'s tests in the reader itself.)

use pdfextractor_image_parser::{
    mimetype, tiff_frames, transcode_to_png, turn_upright, upright_png, webp_frames, ImageDocument,
    Orientation, DEFAULT_DPI,
};
use std::path::PathBuf;

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn bytes(relative: &str) -> Vec<u8> {
    let path = repository_root().join("test-data").join(relative);
    std::fs::read(&path).unwrap_or_else(|e| panic!("missing {}: {e}", path.display()))
}

fn read(relative: &str) -> ImageDocument {
    let bytes = bytes(relative);
    ImageDocument::parse_bytes(&bytes).unwrap_or_else(|e| panic!("{relative}: {e}"))
}

/// The two encodings Docling cannot name are decoded and re-encoded as PNG,
/// and the pixels are the scan's own: the same 620 x 350 grid, byte for byte,
/// under the one media type an `ImageRef` can carry. WebP is `image`'s decoder;
/// JPEG 2000 is the decoder `image` does not have.
#[test]
fn a_webp_and_a_jp2_are_transcoded_to_the_scan_png() {
    let source = image::load_from_memory(&bytes("invoice_scan.png"))
        .expect("the PNG fixture")
        .to_rgba8();

    for file in [
        "images/scan.webp",
        "images/scan.jp2",
        "images/scan.j2c",
        "images/scan.jpc",
        "images/scan.jpf",
    ] {
        let encoded = bytes(file);
        let picture =
            transcode_to_png(&encoded).unwrap_or_else(|| panic!("{file} did not transcode"));
        assert_eq!(
            &picture.data[..8],
            &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
            "{file}: not a PNG"
        );
        assert_eq!((picture.width, picture.height), (620, 350), "{file}");

        // What the transcode must carry: the pixels its own decoder reads. The
        // WebP fixture is lossy (quality 95), so its expected pixels are the
        // WebP's own decode; the JPEG 2000 fixture is lossless, so they are the
        // scan's. Either way the PNG must not be a re-quantised picture.
        let expected = if file.ends_with(".webp") {
            image::load_from_memory(&encoded)
                .expect("the WebP decodes")
                .to_rgba8()
        } else {
            source.clone()
        };
        let pixels = image::load_from_memory(&picture.data)
            .expect("the transcoded PNG decodes")
            .to_rgba8();
        assert_eq!(pixels.dimensions(), expected.dimensions(), "{file}");
        assert!(
            pixels.as_raw() == expected.as_raw(),
            "{file}: the transcode changed the pixels"
        );
    }

    // Bytes that are not a picture are no PNG to invent.
    assert!(transcode_to_png(b"not an image").is_none());
}

/// A TIFF is a stack, not a picture: a scanner's feeder writes one image file
/// directory per sheet, and `pages.tiff` is three of them written that way. The
/// reader gives each as its own PNG at its own grid — the last sheet is a
/// different shape, so a reader that measured the first and reused it cannot
/// pass — while the document still describes the first page, as it does for a
/// GIF.
#[test]
fn a_multipage_tiff_is_one_picture_per_directory() {
    let encoded = bytes("images/pages.tiff");
    let document = ImageDocument::parse_bytes(&encoded).expect("the first page reads");
    assert_eq!(document.mimetype, "image/tiff");
    assert_eq!((document.width, document.height), (480, 160));
    assert_eq!(document.dpi, Some((72.0, 72.0)));

    let pages = tiff_frames(&encoded);
    let sizes: Vec<(u32, u32)> = pages.iter().map(|page| (page.width, page.height)).collect();
    assert_eq!(sizes, vec![(480, 160), (480, 160), (432, 192)], "{sizes:?}");
    for page in &pages {
        assert_eq!(
            &page.data[..8],
            &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
            "a page is not a PNG"
        );
        let pixels = image::load_from_memory(&page.data)
            .expect("the page decodes")
            .to_rgba8();
        assert_eq!(pixels.dimensions(), (page.width, page.height));
    }

    // One directory is one page, and a file that is not a TIFF is no page of
    // pages at all.
    assert_eq!(tiff_frames(&bytes("images/scan.tiff")).len(), 1);
    assert!(tiff_frames(&bytes("images/scan.jpg")).is_empty());
    assert!(tiff_frames(b"not a picture").is_empty());
}

/// An animated WebP is a stack as a GIF is: `turning.webp` is three frames on
/// one canvas, each saying its own words. Every frame is a PNG at the canvas's
/// grid in the order the file plays them; a still WebP is no animation at all,
/// so its one page stays the caller's.
#[test]
fn an_animated_webp_is_one_picture_per_frame() {
    let encoded = bytes("images/turning.webp");
    let document = ImageDocument::parse_bytes(&encoded).expect("the canvas reads");
    assert_eq!(document.mimetype, "image/webp");
    assert_eq!((document.width, document.height), (480, 160));
    assert_eq!(document.dpi, None, "a WebP states no resolution");

    let pages = webp_frames(&encoded);
    assert_eq!(pages.len(), 3, "one page for every frame");
    for page in &pages {
        assert_eq!((page.width, page.height), (480, 160));
        assert_eq!(
            &page.data[..8],
            &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
            "a frame is not a PNG"
        );
        let pixels = image::load_from_memory(&page.data)
            .expect("the frame decodes")
            .to_rgba8();
        assert_eq!(pixels.dimensions(), (480, 160));
        // A frame is its own page, not an empty canvas: the words put ink on
        // what is otherwise white paper.
        assert!(
            pixels.pixels().any(|pixel| pixel.0[0] < 128),
            "the frame drew nothing"
        );
    }

    // One still is not an animation, and another animation is not a WebP.
    assert!(webp_frames(&bytes("images/scan.webp")).is_empty());
    assert!(webp_frames(&bytes("images/turning.gif")).is_empty());
    assert!(webp_frames(b"not a picture").is_empty());
}

/// The page of a 150 dpi scan: 620 x 350 pixels, in points.
const WIDTH_PT: f64 = 297.575;
const HEIGHT_PT: f64 = 167.986;

#[test]
fn every_encoding_is_one_page_of_the_same_size() {
    for (file, mimetype) in [
        ("invoice_scan.png", "image/png"),
        ("images/scan.jpg", "image/jpeg"),
        ("images/scan.tiff", "image/tiff"),
        ("images/scan.bmp", "image/bmp"),
    ] {
        let image = read(file);
        assert_eq!(image.mimetype, mimetype, "{file}");
        assert_eq!((image.width, image.height), (620, 350), "{file}");
        let (x, y) = image.dpi.expect("the encoding declares a resolution");
        assert!(
            (x - 150.0).abs() < 0.1 && (y - 150.0).abs() < 0.1,
            "{file}: {x}x{y}"
        );
        assert!(
            (image.width_pt() - WIDTH_PT).abs() < 0.1,
            "{file}: {}",
            image.width_pt()
        );
        assert!(
            (image.height_pt() - HEIGHT_PT).abs() < 0.1,
            "{file}: {}",
            image.height_pt()
        );
    }
}

/// A JP2 that states its capture resolution is placed at it: the box counts in
/// samples per metre, so the 5906 its header holds is the 150 dpi every other
/// encoding declares, and the 620 pixel wide scan is 297.6 points rather than
/// 620. The codestream is the one `scan.jp2` carries — the box is a header
/// fact — so the pixels are the same too.
#[test]
fn a_jp2_capture_resolution_places_the_page_at_the_scans_dpi() {
    let declared = read("images/scan_150dpi.jp2");
    assert_eq!(declared.mimetype, "image/jp2");
    assert_eq!((declared.width, declared.height), (620, 350));
    let (x, y) = declared.dpi.expect("the capture resolution box");
    assert!(
        (x - 150.0).abs() < 0.1 && (y - 150.0).abs() < 0.1,
        "{x}x{y}"
    );
    assert!(
        (declared.width_pt() - WIDTH_PT).abs() < 0.1,
        "{}",
        declared.width_pt()
    );
    assert!(
        (declared.height_pt() - HEIGHT_PT).abs() < 0.1,
        "{}",
        declared.height_pt()
    );

    // The same picture as the file that declares none: the box says how finely
    // it was captured, not what the pixels are.
    let pixels = |file: &str| {
        transcode_to_png(&bytes(file))
            .unwrap_or_else(|| panic!("{file} did not transcode"))
            .data
    };
    assert_eq!(pixels("images/scan.jp2"), pixels("images/scan_150dpi.jp2"));

    // The same container under `.jpf`: the box is in the bytes, so the page is
    // placed at the resolution its own header declares whatever the name is.
    let named = read("images/scan.jpf");
    assert_eq!(named.mimetype, "image/jp2");
    assert_eq!((named.width, named.height), (620, 350));
    let (x, y) = named.dpi.expect("the capture resolution box");
    assert!(
        (x - 150.0).abs() < 0.1 && (y - 150.0).abs() < 0.1,
        "{x}x{y}"
    );
    assert!(
        (named.width_pt() - WIDTH_PT).abs() < 0.1,
        "{}",
        named.width_pt()
    );
    assert_eq!(pixels("images/scan.jpf"), pixels("images/scan.jp2"));
}

/// A bare codestream is the same scan as its JP2: the container states the page
/// in `ihdr`, the codestream in `SIZ`, and both say 620 x 350. The codestream
/// states no density — the `res` box is a container box and was left behind —
/// so its page is one pixel per point, while the JP2 it came from is too because
/// that fixture carries no `resc` box either.
#[test]
fn a_codestream_is_the_same_scan_page_as_its_jp2() {
    let jp2 = read("images/scan.jp2");
    let codestream = read("images/scan.j2c");
    assert_eq!(codestream.mimetype, "image/jp2");
    assert_eq!((codestream.width, codestream.height), (620, 350));
    assert_eq!(codestream.dpi, None);
    assert_eq!(
        (codestream.width_pt(), codestream.height_pt()),
        (jp2.width_pt(), jp2.height_pt())
    );
    assert_eq!(
        (codestream.width_pt(), codestream.height_pt()),
        (620.0, 350.0)
    );

    // The pixels are the container's own codestream: the decoder reads both and
    // has to agree byte for byte.
    let pixels = |file: &str| {
        transcode_to_png(&bytes(file))
            .unwrap_or_else(|| panic!("{file} did not transcode"))
            .data
    };
    assert_eq!(pixels("images/scan.j2c"), pixels("images/scan.jp2"));

    // `.jpc` names the same codestream and reads the same way, because the name
    // is the only thing that differs: the bytes are `scan.j2c`'s.
    let other_name = read("images/scan.jpc");
    assert_eq!(other_name.mimetype, "image/jp2");
    assert_eq!((other_name.width, other_name.height), (620, 350));
    assert_eq!(other_name.dpi, None);
    assert_eq!(pixels("images/scan.jpc"), pixels("images/scan.j2c"));
}

/// The name a JPEG 2000 is saved under does not decide which header is read:
/// `.jpc` and `.jpf` are both spellings of the same format, and each file's own
/// first bytes say whether it opens with a JP2 signature box or a codestream's
/// `FF 4F FF 51` start marker. The fixtures are crossed on purpose — `.jpf`
/// holds the container here — so a reader that keyed on the extension would
/// state the wrong page: 620 × 350 at the 72 dpi default instead of the 150 dpi
/// the file's `resc` box declares.
#[test]
fn the_name_of_a_jpeg_2000_does_not_decide_its_header() {
    let container = read("images/scan.jpf");
    let codestream = read("images/scan.jpc");

    // `.jpf` is the container: its `ihdr` and capture-resolution boxes state
    // the page, so it is the 150 dpi scan however it is named.
    assert_eq!(container.mimetype, "image/jp2");
    assert_eq!((container.width, container.height), (620, 350));
    assert_eq!(
        container.dpi.map(|(x, _)| x.round()),
        Some(150.0),
        ".jpf was read as a codestream rather than as the container it holds"
    );

    // `.jpc` is the bare codestream: its `SIZ` marker states the grid and it
    // has no container box to declare a density.
    assert_eq!(codestream.mimetype, "image/jp2");
    assert_eq!((codestream.width, codestream.height), (620, 350));
    assert_eq!(codestream.dpi, None);
    assert_eq!(codestream.width_pt(), 620.0);

    // And the reader's own type sniffing agrees with the headers, not the names.
    assert_eq!(mimetype(&bytes("images/scan.jpc")), Some("image/jp2"));
    assert_eq!(mimetype(&bytes("images/scan.jpf")), Some("image/jp2"));
}

/// Every prefix of the fixtures is refused, none of them panics.
///
/// A box parser walks lengths the file states about itself, and a marker walk
/// does the same with the lengths in each marker, so a truncated file is the
/// input that finds an unchecked slice: every prefix of `scan.jp2`, of the copy
/// with a resolution box, and of the bare codestream is handed to the reader and
/// has to come back as a page or an error.
#[test]
fn a_truncated_jp2_is_never_a_panic() {
    for file in [
        "images/scan.jp2",
        "images/scan_150dpi.jp2",
        "images/scan.j2c",
        "images/scan.jpc",
        "images/scan.jpf",
    ] {
        let encoded = bytes(file);
        for end in 0..encoded.len() {
            let _ = ImageDocument::parse_bytes(&encoded[..end]);
        }
        assert!(ImageDocument::parse_bytes(&encoded).is_ok(), "{file}");
    }
}

/// The encodings that state no resolution: a WebP has no field for one, a JP2
/// states none unless it carries a capture-resolution box, a JPEG 2000
/// codestream has no container to hold one, and a GIF's screen descriptor states
/// a pixel aspect ratio rather than a density. All of them are then one pixel
/// per point, the default Docling's own backend takes.
#[test]
fn a_webp_a_jp2_and_a_gif_with_no_resolution_are_one_pixel_per_point() {
    for (file, mimetype) in [
        ("images/scan.webp", "image/webp"),
        ("images/scan.jp2", "image/jp2"),
        ("images/scan.j2c", "image/jp2"),
        ("images/scan.jpc", "image/jp2"),
        ("images/scan.gif", "image/gif"),
    ] {
        let image = read(file);
        assert_eq!(image.mimetype, mimetype, "{file}");
        assert_eq!((image.width, image.height), (620, 350), "{file}");
        assert_eq!(image.dpi, None, "{file}");
        assert_eq!(DEFAULT_DPI, 72.0);
        assert_eq!(
            (image.width_pt(), image.height_pt()),
            (620.0, 350.0),
            "{file}"
        );
    }
}

#[test]
fn the_page_is_the_pixel_grid_divided_by_the_declared_resolution() {
    let image = read("invoice_scan.png");
    let (x, _) = image.dpi.expect("a declared resolution");
    assert_eq!(image.width_pt(), 620.0 * 72.0 / x);
    assert_eq!(image.height_pt(), 350.0 * 72.0 / x);
}

/// A phone held sideways: the photographed corpus stores the invoice as a
/// landscape grid and tags it EXIF 6, so the page is the portrait one the tag
/// describes, not the grid the bytes hold.
#[test]
fn a_camera_sideways_scan_is_a_portrait_page() {
    let image = read("photos/06_rotated.jpg");
    assert_eq!(image.mimetype, "image/jpeg");
    assert_eq!(image.orientation, Orientation::Rotate90);
    assert_eq!((image.width, image.height), (1754, 1240));
    assert_eq!(image.upright_size(), (1240, 1754));
    assert!(
        (image.width_pt() - 595.2).abs() < 0.1,
        "{}",
        image.width_pt()
    );
    assert!(
        (image.height_pt() - 841.92).abs() < 0.1,
        "{}",
        image.height_pt()
    );
}

/// The clean scan carries no orientation tag, and is read as stored.
#[test]
fn an_image_with_no_orientation_tag_is_read_as_stored() {
    let image = read("invoice_scan.png");
    assert_eq!(image.orientation, Orientation::Normal);
    assert_eq!(image.upright_size(), (620, 350));
}

/// A picture a camera stored sideways, turned into the picture the tag says it
/// is: the dimensions swap, the bytes decode upright, and the tag is gone
/// because the pixels no longer need it.
#[test]
fn a_tagged_picture_is_turned_and_re_encoded_upright() {
    let stored = bytes("photos/06_rotated.jpg");
    let upright = turn_upright(&stored).expect("the tag says it is sideways");

    assert_eq!(upright.mimetype, "image/jpeg", "a JPEG stays a JPEG");
    assert_eq!((upright.width, upright.height), (1240, 1754));
    assert!(
        !upright.data.windows(6).any(|window| window == b"Exif\0\0"),
        "the tag must not survive the turn"
    );
    // The bytes themselves are upright, not merely reported as such.
    let decoded = image::load_from_memory(&upright.data).expect("the encoded picture decodes");
    assert_eq!((decoded.width(), decoded.height()), (1240, 1754));
}

/// A picture with no tag is already the picture: turning it would be a guess,
/// so it is left byte for byte alone.
#[test]
fn a_picture_without_a_tag_is_not_touched() {
    assert!(turn_upright(&bytes("invoice_scan.png")).is_none());
    // Stored sideways, but nothing in the file says so: not this function's
    // call to make, and not an excuse to re-encode it.
    assert!(turn_upright(&bytes("photos/07_sideways.jpg")).is_none());
}

/// The form an OCR engine needs: lossless PNG pixels, upright, with the pixel
/// grid the turn produces.
#[test]
fn the_ocr_form_is_a_png_of_the_upright_pixels() {
    let png = upright_png(&bytes("photos/06_rotated.jpg"), Orientation::Rotate90)
        .expect("the pixels can be decoded");
    assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    let decoded = image::load_from_memory(&png).expect("the PNG decodes");
    assert_eq!((decoded.width(), decoded.height()), (1240, 1754));
    // Upright already: nothing to turn.
    assert!(upright_png(&bytes("photos/06_rotated.jpg"), Orientation::Normal).is_none());
}
