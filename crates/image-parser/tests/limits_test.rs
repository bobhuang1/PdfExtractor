use pdfextractor_image_parser::{upright_png_checked, ImageDocument, Orientation};
use pdfextractor_limits::{LimitError, Limits};

/// A PNG signature and IHDR big enough for the header reader, with a grid that
/// would be many gigabytes if a decoder trusted it.
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    out.extend_from_slice(&13u32.to_be_bytes());
    out.extend_from_slice(b"IHDR");
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&height.to_be_bytes());
    out.extend_from_slice(&[8, 6, 0, 0, 0]);
    out.extend_from_slice(&[0, 0, 0, 0]); // CRC: the header reader does not verify it
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(b"IEND");
    out.extend_from_slice(&[0, 0, 0, 0]);
    out
}

/// A frame stack is decoded frame by frame, and each frame is checked against
/// the deadline: a stack that is inside the frame budget but too slow to read
/// is refused between pages rather than allowed to run on.
#[test]
fn a_frame_stack_over_its_time_budget_is_refused() {
    let data =
        std::fs::read("../../test-data/images/pages.tiff").expect("the multi-page TIFF fixture");
    let limits = Limits {
        timeout: std::time::Duration::ZERO,
        ..Limits::default()
    };
    let error = pdfextractor_image_parser::tiff_frames_checked(&data, &limits)
        .expect_err("the deadline is spent");
    assert!(
        matches!(error, LimitError::Timeout { .. }),
        "unexpected refusal: {error}"
    );
    assert!(error.to_string().contains("TIFF page"), "{error}");
}

#[test]
fn a_header_that_claims_a_gigantic_grid_is_refused_before_decoding() {
    let data = png(50_000, 50_000);
    let limits = Limits {
        max_image_pixels: 10_000_000,
        ..Limits::default()
    };
    let error = ImageDocument::parse_bytes_with_limits(&data, &limits).unwrap_err();
    let text = error.to_string();
    assert!(text.contains("pixels"), "unexpected refusal: {text}");
}

#[test]
fn a_side_over_the_dimension_ceiling_is_refused() {
    let data = png(1_000, 1_000);
    let limits = Limits {
        max_image_dimension: 500,
        ..Limits::default()
    };
    let error = ImageDocument::parse_bytes_with_limits(&data, &limits).unwrap_err();
    assert!(error.to_string().contains("500"));
}

#[test]
fn the_default_budget_accepts_a_letter_sized_scan() {
    let data = png(2_550, 3_300);
    let document = ImageDocument::parse_bytes(&data).expect("a normal scan");
    assert_eq!((document.width, document.height), (2_550, 3_300));
}

/// A nominally sized, decodable picture: the header reader and the decoder both
/// accept it, so only a budget can stand between it and its pixels.
fn decodable(width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();
    image::GrayImage::from_pixel(width, height, image::Luma([0x7f]))
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .expect("encode the test PNG");
    out
}

/// The EXIF turn is one more decode, so it takes the caller's budgets: a
/// lowered ceiling is refused with the budget's own error (not `None`), and at
/// the defaults the same picture comes back with its axes swapped.
#[test]
fn the_exif_turn_is_held_to_the_callers_budget_and_returns_upright_pixels() {
    let data = decodable(4, 2);
    let tight = Limits {
        max_image_pixels: 1,
        ..Limits::default()
    };
    let error = upright_png_checked(&data, Orientation::Rotate90, &tight)
        .expect_err("a four-by-two picture must be refused by a one-pixel budget");
    assert!(
        matches!(error, LimitError::ImageTooManyPixels { .. }),
        "the refusal must be the budget's own error, not a decode failure: {error}"
    );

    let png = upright_png_checked(&data, Orientation::Rotate90, &Limits::default())
        .expect("the defaults turn a tiny page");
    let turned = image::load_from_memory(&png).expect("the turn writes a decodable PNG");
    assert_eq!(
        (turned.width(), turned.height()),
        (2, 4),
        "a quarter turn must swap the page's axes"
    );
}

#[test]
fn the_frame_count_budget_is_a_named_refusal() {
    let limits = Limits {
        max_image_frames: 2,
        ..Limits::default()
    };
    let error = limits.check_image_frames(3).unwrap_err();
    assert!(matches!(
        error,
        LimitError::TooManyImageFrames {
            actual: 3,
            limit: 2
        }
    ));
}
