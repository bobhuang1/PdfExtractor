//! The environment's budget overrides reach `limits()`.
//!
//! The targets take their budgets from the environment, so a variable `limits()`
//! never reads leaves a lowered run at the product defaults — a failure no
//! target can report, because it looks like an ordinary run. That hop is what
//! this test holds: set the variable, read the budgets back, clear it again.
//!
//! The rest of the chain — that the picture transcode refuses a grid past
//! `max_image_pixels` — is held by
//! `transcode_to_png_with_limits_refuses_a_grid_the_defaults_decode` in
//! `image-parser`, where the budget is built directly; this test does not
//! re-prove it, and could not see whether the image target passes its budget,
//! since the target's body only runs under libFuzzer.

use pdfextractor_fuzz::limits;
use pdfextractor_limits::Limits;

#[test]
fn a_set_variable_lowers_its_budget_and_an_unset_one_keeps_the_default() {
    let defaults = Limits::default();
    assert_ne!(
        defaults.max_image_pixels, 1,
        "the value below must differ from the product default, or this test would pass without reading the environment"
    );

    // `limits()` reads the process environment, and this is the binary's only
    // test, so the variable is set and cleared around the one read rather than
    // left for whatever runs next.
    std::env::set_var("FUZZ_MAX_IMAGE_PIXELS", "1");
    let budget = limits();
    std::env::remove_var("FUZZ_MAX_IMAGE_PIXELS");

    assert_eq!(
        budget.max_image_pixels, 1,
        "limits() did not take FUZZ_MAX_IMAGE_PIXELS from the environment"
    );
    assert_eq!(
        budget.max_image_frames, defaults.max_image_frames,
        "a variable the run did not set must keep its product default"
    );
}
