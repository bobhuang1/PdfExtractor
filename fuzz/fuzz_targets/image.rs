#![no_main]

use libfuzzer_sys::fuzz_target;
use pdfextractor_fuzz::{limits, note};
use pdfextractor_image_parser::{
    gif_frames_checked, tiff_frames_checked, transcode_to_png_checked, webp_frames_checked,
    ImageDocument,
};

fuzz_target!(|data: &[u8]| {
    let budget = limits();
    note(
        "image",
        &ImageDocument::parse_bytes_with_limits(data, &budget),
    );
    // Checked rather than the `Option` form: a lowered pixel or allocation
    // ceiling is a refusal to report, not a picture that quietly came back empty.
    note("transcode", &transcode_to_png_checked(data, &budget));
    // Each checked reader returns an empty list for a non-matching format, so
    // all three can be called on every input without a discriminator byte.
    note("gif", &gif_frames_checked(data, &budget));
    note("webp", &webp_frames_checked(data, &budget));
    note("tiff", &tiff_frames_checked(data, &budget));
});
