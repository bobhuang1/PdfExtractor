//! The two PDFs the `pdf` fuzz target crashed on, replayed as tests.
//!
//! libFuzzer saves a finding under `fuzz/artifacts/<target>/`; these two are
//! committed (an explicit exception in `fuzz/.gitignore`) so the crash stays
//! covered without a nightly toolchain or the machine that found it. Each is a
//! `%PDF-1.4` whose second page draws a 32x32 `/DeviceRGB` image XObject whose
//! stream holds more samples than 32 × 32 × 3: the PNG encoder asserts the
//! exact grid length, so an over-long buffer used to abort the process here.

use std::fs;
use std::path::PathBuf;

use pdfextractor_pdf_parser::PdfDocument;

/// The exact bytes libFuzzer wrote under `fuzz/artifacts/pdf/`.
const ARTIFACTS: [&str; 2] = [
    "crash-5ab64f5f121aa960e309bd2185d4263d6bab8a6d",
    "crash-b6e2decaa61d0829f9a5e149d256bf95e8017915",
];

fn artifact(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fuzz/artifacts/pdf")
        .join(name);
    fs::read(&path).unwrap_or_else(|error| panic!("could not read {}: {error}", path.display()))
}

/// The bytes of the image stream the artifact carries, which it leaves
/// unfiltered and uncompressed.
fn image_stream(data: &[u8]) -> &[u8] {
    const IMAGE: &[u8] = b"/Subtype /Image";
    const STREAM: &[u8] = b"stream\n";
    const END: &[u8] = b"endstream";
    let image = data
        .windows(IMAGE.len())
        .position(|window| window == IMAGE)
        .expect("the artifact states an image XObject");
    let start = image
        + data[image..]
            .windows(STREAM.len())
            .position(|window| window == STREAM)
            .expect("the image object has a stream")
        + STREAM.len();
    let end = start
        + data[start..]
            .windows(END.len())
            .position(|window| window == END)
            .expect("the image stream ends");
    // The end-of-line before `endstream` is not part of the stream.
    match data[start..end].last() {
        Some(b'\n') => &data[start..end - 1],
        _ => &data[start..end],
    }
}

#[test]
fn the_fuzzed_rgb_streams_longer_than_their_grid_are_cut_to_the_picture() {
    for name in ARTIFACTS {
        let data = artifact(name);
        let stream = image_stream(&data);
        assert!(
            stream.len() > 32 * 32 * 3,
            "{name}: the stream holds {} samples, not more than the 32x32x3 grid",
            stream.len()
        );

        let doc = PdfDocument::parse(&data).unwrap_or_else(|error| panic!("{name}: {error}"));
        let pages = doc
            .extract_checked()
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(pages.len(), 2, "{name}: the artifact has two pages");
        assert_eq!(
            pages[1].images.len(),
            1,
            "{name}: the picture page decodes its image XObject"
        );

        let picture = &pages[1].images[0];
        assert_eq!(picture.mime, "image/png", "{name}");
        assert_eq!((picture.width, picture.height), (32, 32), "{name}");
        assert_eq!(
            &picture.data[..8],
            &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a],
            "{name}: the picture is not PNG data"
        );

        let decoded = image::load_from_memory(&picture.data)
            .unwrap_or_else(|error| panic!("{name}: the written PNG decodes: {error}"))
            .to_rgb8();
        assert_eq!(decoded.dimensions(), (32, 32), "{name}");
    }
}
