//! The environment-driven budgets reach the picture transcode.
//!
//! The image target builds its `Limits` with [`limits`] and hands them to
//! `transcode_to_png_with_limits`. That is two hops a unit test of the decoder
//! alone would not hold together, so this one takes the same two: set
//! `FUZZ_MAX_IMAGE_PIXELS`, read it back through [`limits`], and check the
//! picture the defaults decode is refused under the lowered budget.

use std::path::PathBuf;

use pdfextractor_fuzz::limits;
use pdfextractor_image_parser::{transcode_to_png, transcode_to_png_with_limits};

#[test]
fn a_lowered_pixel_budget_refuses_the_transcode_of_an_oversize_picture() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../test-data/invoice_scan.png");
    let picture = std::fs::read(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    assert!(
        transcode_to_png(&picture).is_some(),
        "the product defaults must decode the committed picture"
    );

    // `limits()` reads the process environment, and this is the only test in
    // the crate, so the variable is set and cleared around the one read rather
    // than left for whatever runs next.
    std::env::set_var("FUZZ_MAX_IMAGE_PIXELS", "1");
    let budget = limits();
    std::env::remove_var("FUZZ_MAX_IMAGE_PIXELS");

    assert_eq!(
        budget.max_image_pixels, 1,
        "limits() did not take FUZZ_MAX_IMAGE_PIXELS from the environment"
    );
    assert!(
        transcode_to_png_with_limits(&picture, &budget).is_none(),
        "a one-pixel budget must refuse an over-size picture"
    );
}
