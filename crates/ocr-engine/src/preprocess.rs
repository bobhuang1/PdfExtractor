//! Despeckle: the defect a faxed page has that a born-digital page does not.
//!
//! A fax line that drops paints isolated black pixels on white paper, or white
//! ones inside ink. Tesseract copes with a little noise, but it costs: a speck
//! between two letters joins them into a word that is not there, and a speck
//! inside a glyph splits it. Replacing the pixels no neighbour agrees with,
//! before recognition, is cheaper than asking the recogniser to be tolerant.
//!
//! **What this is not.** It does not threshold, binarise, denoise a photograph
//! or remove a background. Those change the pixels Tesseract reads, and its own
//! binning is better than a naive threshold. This pass only replaces a pixel
//! that is isolated from every neighbour, so a page without the defect comes
//! through it unchanged.
//!
//! **Deskew is not here yet.** The plan's first OCR item asks for deskew and
//! despeckle together. Despeckle is the half whose result is a local decision —
//! a pixel either has agreeing neighbours or it does not — and it is measured
//! by the tests below. Skew is the half that needs an estimator, and both
//! candidate estimators were measured on rendered fixtures and did not hold:
//! the row-projection profile peaked at zero on a page whose ink centres
//! provably descend, and the centre-fit slope read half the true turn on a
//! multi-line page. Shipping a rotation on either would move every scan's
//! pixels on an unvalidated reading, so deskew stays open with those
//! measurements recorded in `plan.md`.
//!
//! **Budgets.** The decode is held to the run's image limits like every other
//! decode, and the pass works on a grey copy of the page, so it costs one byte
//! per pixel beyond the decoded image itself.

use anyhow::{Context, Result};
use image::{GrayImage, Luma};
use pdfextractor_limits::Limits;

/// A dark speck: darker than this, and every neighbour lighter than
/// [`SPECK_LIGHT`]. Values come from a fax's two-tone output, which leaves
/// pixels far from the paper and far from the ink, never in between.
const SPECK_DARK: u8 = 55;
/// A light speck: lighter than this, with every neighbour darker than
/// [`SPECK_DARK`].
const SPECK_LIGHT: u8 = 200;

/// Replace the specks on an encoded page, returning PNG bytes for
/// [`crate::OcrEngine::recognize`].
///
/// The page keeps its pixel grid — the same width, height and orientation — so
/// a caller scales the boxes that come back exactly as it scaled the ones from
/// the untouched image.
///
/// `limits` are the run's image budgets: the page is decoded under them, so a
/// page they refuse is refused here with the budget's own
/// [`pdfextractor_limits::LimitError`] rather than half-processed.
pub fn despeckle_page(image_bytes: &[u8], limits: &Limits) -> Result<Vec<u8>> {
    let bitmap = pdfextractor_image_parser::decode_checked(image_bytes, limits)
        .context("failed to decode a page for despeckling")?;
    let mut gray = bitmap.to_luma8();
    despeckle(&mut gray);

    let mut png = Vec::new();
    image::DynamicImage::ImageLuma8(gray)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .context("failed to encode a despeckled page")?;
    Ok(png)
}

/// Replace every pixel its neighbours unanimously disagree with.
///
/// This is the fax defect exactly: a dropped line paints one pixel black on
/// white paper, or one white on black ink. Text strokes are several pixels
/// wide, so a pixel inside a stroke always has a neighbour as dark as itself,
/// and the pass cannot thin a glyph. A photograph's grain fails the same test —
/// its neighbours vary — so this leaves it alone rather than smoothing it.
fn despeckle(image: &mut GrayImage) {
    let (width, height) = image.dimensions();
    let source = image.clone();
    for y in 0..height {
        for x in 0..width {
            let value = source.get_pixel(x, y)[0];
            let dark_speck = value < SPECK_DARK;
            let light_speck = value > SPECK_LIGHT;
            if !dark_speck && !light_speck {
                continue;
            }

            let mut neighbours = 0u32;
            let mut sum = 0u32;
            let mut all_light = true;
            let mut all_dark = true;
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let nx = x as i64 + dx;
                    let ny = y as i64 + dy;
                    if nx < 0 || ny < 0 || nx >= width as i64 || ny >= height as i64 {
                        continue;
                    }
                    let neighbour = source.get_pixel(nx as u32, ny as u32)[0];
                    neighbours += 1;
                    sum += neighbour as u32;
                    all_light &= neighbour > SPECK_LIGHT;
                    all_dark &= neighbour < SPECK_DARK;
                }
            }
            if neighbours == 0 {
                continue;
            }
            // A dark speck on paper, or a light one inside ink: either way the
            // neighbours agree, and their value replaces it.
            if (dark_speck && all_light) || (light_speck && all_dark) {
                image.put_pixel(x, y, Luma([(sum / neighbours) as u8]));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(image: &GrayImage) -> Vec<u8> {
        let mut png = Vec::new();
        image::DynamicImage::ImageLuma8(image.clone())
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("encode a fixture");
        png
    }

    fn decode(png: &[u8]) -> GrayImage {
        image::load_from_memory(png).expect("decoded").to_luma8()
    }

    #[test]
    fn a_speck_is_removed_and_a_stroke_is_kept() {
        let mut image = GrayImage::from_pixel(40, 40, Luma([255]));
        for x in 5..35 {
            image.put_pixel(x, 20, Luma([0]));
        }
        image.put_pixel(2, 2, Luma([0]));
        image.put_pixel(37, 7, Luma([0]));

        despeckle(&mut image);

        assert_eq!(image.get_pixel(2, 2)[0], 255, "an isolated pixel survived");
        assert_eq!(
            image.get_pixel(37, 7)[0],
            255,
            "a second isolated pixel survived"
        );
        assert_eq!(
            image.get_pixel(20, 20)[0],
            0,
            "the stroke was thinned: a bar is not a speck"
        );
    }

    #[test]
    fn a_light_speck_inside_ink_is_removed() {
        let mut image = GrayImage::from_pixel(40, 40, Luma([0]));
        image.put_pixel(20, 20, Luma([255]));

        despeckle(&mut image);

        assert_eq!(
            image.get_pixel(20, 20)[0],
            0,
            "a white pixel inside black ink survived"
        );
    }

    #[test]
    fn a_grainy_photograph_is_left_alone() {
        // Grain is not a speck: neighbours disagree with each other, so no
        // pixel meets the test and the photograph is not smoothed.
        let mut image = GrayImage::from_pixel(12, 12, Luma([128]));
        for (x, y) in [(3u32, 3u32), (5, 8), (9, 2)] {
            image.put_pixel(x, y, Luma([0]));
            image.put_pixel(x + 1, y, Luma([200]));
        }
        let before = image.clone();

        despeckle(&mut image);

        assert_eq!(image, before, "a grainy patch was altered");
    }

    #[test]
    fn the_page_keeps_its_grid_and_loses_its_specks() {
        let mut image = GrayImage::from_pixel(120, 90, Luma([255]));
        for x in 10..110 {
            image.put_pixel(x, 40, Luma([0]));
        }
        let dark = [(5u32, 5u32), (60, 80), (115, 12)];
        for (x, y) in dark {
            image.put_pixel(x, y, Luma([0]));
        }

        let prepared = despeckle_page(&encode(&image), &Limits::default()).expect("despeckle");
        let decoded = decode(&prepared);

        assert_eq!(
            (decoded.width(), decoded.height()),
            (120, 90),
            "the grid changed, so the caller's boxes no longer fit"
        );
        for (x, y) in dark {
            assert_eq!(
                decoded.get_pixel(x, y)[0],
                255,
                "the speck at ({x}, {y}) survived the page pass"
            );
        }
        assert_eq!(decoded.get_pixel(60, 40)[0], 0, "the stroke was lost");
    }
}
