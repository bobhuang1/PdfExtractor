#![no_main]

use libfuzzer_sys::fuzz_target;
use pdfextractor_fuzz::{limits, note};
use pdfextractor_image_parser::{
    gif_frames_checked, tiff_frames_checked, transcode_to_png_with_limits, webp_frames_checked,
    ImageDocument,
};

fuzz_target!(|data: &[u8]| {
    let budget = limits();
    note(
        "image",
        &ImageDocument::parse_bytes_with_limits(data, &budget),
    );
    let _ = transcode_to_png_with_limits(data, &budget);
    // Each checked reader returns an empty list for a non-matching format, so
    // all three can be called on every input without a discriminator byte.
    note("gif", &gif_frames_checked(data, &budget));
    note("webp", &webp_frames_checked(data, &budget));
    note("tiff", &tiff_frames_checked(data, &budget));
});
