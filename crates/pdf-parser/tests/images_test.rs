use pdfextractor_pdf_parser::PdfDocument;
use std::fs;

/// A page whose scan is CCITT: the first page Group 3 one-dimensional, the
/// second Group 4. Both streams come from the same thresholded invoice, so
/// both must decode to the same pixels, at the grid the TIFF strips state.
#[test]
fn decodes_ccitt_group3_and_group4_scans() {
    let data = fs::read("../../test-data/ccitt_scan.pdf").expect("read ccitt_scan.pdf");
    let doc = PdfDocument::parse(&data).expect("parse");
    let pages = doc.extract();
    assert_eq!(pages.len(), 2, "the fixture is one page per codec");

    let decoded: Vec<image::GrayImage> = pages
        .iter()
        .map(|page| {
            assert_eq!(page.images.len(), 1, "one CCITT image XObject per page");
            let img = &page.images[0];
            assert_eq!(img.mime, "image/png");
            assert_eq!((img.width, img.height), (620, 350));
            assert_eq!(
                &img.data[..8],
                &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a],
                "the picture is not PNG data"
            );
            image::load_from_memory(&img.data)
                .expect("the written PNG decodes")
                .to_luma8()
        })
        .collect();

    for (index, picture) in decoded.iter().enumerate() {
        assert_eq!(picture.dimensions(), (620, 350), "picture {index}");
        let first = picture.get_pixel(0, 0);
        assert!(
            picture.pixels().any(|pixel| pixel != first),
            "picture {index} is one flat colour"
        );
    }
    // Group 3 and Group 4 are two spellings of the same image.
    assert_eq!(decoded[0], decoded[1], "G3 and G4 decoded differently");
}

/// A Group 3 two-dimensional stream (`/K 1`) written by Ghostscript's
/// `tiffg32d` device. The hand-built bitstream in `images.rs` covers the code
/// units; this fixture proves the parser selects the two-dimensional mode for a
/// real PDF image stream and that the whole page reaches the ordinary CCITT
/// path.
#[test]
fn decodes_ccitt_group3_two_dimensional_scan() {
    let data = fs::read("../../test-data/ccitt_g3_2d_scan.pdf").expect("read ccitt_g3_2d_scan.pdf");
    assert!(
        data.windows(4).any(|window| window == b"/K 1"),
        "the fixture must exercise a positive /K"
    );
    let doc = PdfDocument::parse(&data).expect("parse");
    let pages = doc.extract();
    assert_eq!(pages.len(), 1, "one page");
    assert_eq!(pages[0].images.len(), 1, "one CCITT image XObject");

    let img = &pages[0].images[0];
    assert_eq!(img.mime, "image/png");
    assert_eq!((img.width, img.height), (620, 350));
    assert_eq!(
        &img.data[..8],
        &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a],
        "the picture is not PNG data"
    );
    let decoded = image::load_from_memory(&img.data)
        .expect("the written PNG decodes")
        .to_luma8();
    assert_eq!(decoded.dimensions(), (620, 350));
    let first = decoded.get_pixel(0, 0);
    assert!(
        decoded.pixels().any(|pixel| pixel != first),
        "the Group 3 two-dimensional page is one flat colour"
    );
}

/// A `/JBIG2Decode` image with its symbol dictionary in `/JBIG2Globals`.
/// The page-information segment supplies the 620 x 350 grid; the pixels are
/// the scan, not a blank page.
#[test]
fn decodes_jbig2_pictures_with_globals() {
    let data = fs::read("../../test-data/jbig2_scan.pdf").expect("read jbig2_scan.pdf");
    let doc = PdfDocument::parse(&data).expect("parse");
    let pages = doc.extract();
    assert_eq!(pages.len(), 1, "one page");
    assert_eq!(pages[0].images.len(), 1, "one JBIG2 image XObject");

    let img = &pages[0].images[0];
    assert_eq!(img.mime, "image/png");
    assert_eq!((img.width, img.height), (620, 350));
    let decoded = image::load_from_memory(&img.data)
        .expect("the written PNG decodes")
        .to_luma8();
    let first = decoded.get_pixel(0, 0);
    assert!(
        decoded.pixels().any(|pixel| pixel != first),
        "the JBIG2 page is one flat colour"
    );
}

#[test]
fn extracts_embedded_jpeg_from_scanned_page() {
    let data = fs::read("../../test-data/scanned.pdf").expect("read scanned.pdf");
    let doc = PdfDocument::parse(&data).expect("parse");
    let pages = doc.extract();

    assert_eq!(pages.len(), 1, "one page");
    assert!(pages[0].spans.is_empty(), "a scan has no text layer");

    assert_eq!(
        pages[0].images.len(),
        1,
        "expected exactly one image XObject, got {}",
        pages[0].images.len()
    );
    let img = &pages[0].images[0];
    assert_eq!(img.mime, "image/jpeg");
    assert_eq!(&img.data[..2], &[0xFF, 0xD8], "JPEG SOI marker");
    assert_eq!(img.width, 1275);
    assert_eq!(img.height, 1650);
    assert!(
        img.data.len() > 10_000,
        "jpeg payload should be substantial"
    );
}

/// A page whose pictures are JPEG 2000s. PDF states one as `/Filter
/// /JPXDecode`, and the stream behind that filter is either of the two things a
/// `.jp2` file is made of: the container with its signature and `jp2h` boxes
/// around the codestream, or the bare codestream. Neither can be named in a
/// Docling `image` reference, so both are decoded here and the page's pictures
/// are PNGs at the codestream's own grid — not the box with no image they used
/// to be, and not the file's `/Width` and `/Height` taken on trust.
#[test]
fn decodes_jpx_pictures_into_pngs() {
    let data = fs::read("../../test-data/jpx_picture.pdf").expect("read jpx_picture.pdf");
    let doc = PdfDocument::parse(&data).expect("parse");
    let pages = doc.extract();

    assert_eq!(pages.len(), 1, "one page");
    assert_eq!(
        pages[0].placed_images.len(),
        2,
        "the page draws two pictures"
    );
    assert_eq!(pages[0].images.len(), 2, "and both are decoded");

    for (index, img) in pages[0].images.iter().enumerate() {
        assert_eq!(img.mime, "image/png", "picture {index}");
        assert_eq!(
            &img.data[..8],
            &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a],
            "picture {index} is named a PNG and is not one"
        );
        // The grid is the codestream's: 620 x 350, as the JP2 states it.
        assert_eq!((img.width, img.height), (620, 350), "picture {index}");
    }

    // The two streams are the same drawing in its two forms, so they decode to
    // the same pixels — byte for byte, the encoder being one.
    assert_eq!(pages[0].images[0].data, pages[0].images[1].data);

    // And those pixels are the scan, not a blank page: the decode put ink down.
    let decoded = image::load_from_memory(&pages[0].images[0].data)
        .expect("the written PNG decodes")
        .to_rgb8();
    assert_eq!(decoded.dimensions(), (620, 350));
    let first = decoded.get_pixel(0, 0);
    assert!(
        decoded.pixels().any(|pixel| pixel != first),
        "the picture is one flat colour"
    );
}
