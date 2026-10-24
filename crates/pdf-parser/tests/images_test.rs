use pdfextractor_pdf_parser::PdfDocument;
use std::fs;

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
