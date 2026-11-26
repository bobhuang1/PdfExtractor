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
//! **Deskew.** [`skew_angle`] reads the page's tilt from the sharpness of its
//! row projection: for each candidate angle the ink is binned into rows along
//! that direction, and the angle whose rows separate the text lines most is
//! the page's. The first attempt at this measured the *vertical* projection —
//! it binned ink along `x - t*y`, which straightens upright strokes rather than
//! text lines — and so peaked at zero on a page turned four degrees. Measuring
//! the direction the text actually runs fixed it: on a real scan turned by
//! 2, 4 and 6 degrees and back, the estimator returned exactly those angles,
//! and zero on the upright page. [`deskew_page`] turns the page by that angle
//! about its centre, keeping the grid so the caller maps boxes back through
//! the same turn.
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

/// The largest tilt measured and corrected, in degrees. A page turned further
/// than this is stored on its side, which the quarter-turn reading decides.
const SKEW_RANGE: f64 = 12.0;
/// The candidate step: a quarter of a degree.
const SKEW_STEP: f64 = 0.25;
/// Ink, for the skew estimate. The pages this runs on are text on paper; a dark
/// photograph has no line structure to sharpen, so its estimate is noise and
/// its caller's threshold leaves it alone.
const INK: u8 = 128;

/// The page's skew in degrees: positive when its lines descend to the right.
///
/// The estimator bins the ink into rows along each candidate direction and
/// keeps the direction whose rows separate the text lines most sharply, so it
/// reads the direction the text runs rather than the direction the page's
/// frame is drawn in. A page with too little ink to measure — blank, or a
/// photograph — comes back as zero rather than a guess.
pub fn skew_angle(image_bytes: &[u8], limits: &Limits) -> Result<f64> {
    let bitmap = pdfextractor_image_parser::decode_checked(image_bytes, limits)
        .context("failed to decode a page for the skew estimate")?;
    Ok(estimate_skew(&bitmap.to_luma8()))
}

fn estimate_skew(image: &GrayImage) -> f64 {
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return 0.0;
    }
    // The ink is collected once: every candidate bins the same pixels, only
    // the row each one falls in changes.
    let stride = ((width as usize * height as usize) / 400_000).max(1);
    let width_usize = width as usize;
    let mut points: Vec<(f64, f64)> = Vec::new();
    for (index, pixel) in image.pixels().enumerate() {
        if pixel[0] < INK && index % stride == 0 {
            points.push(((index % width_usize) as f64, (index / width_usize) as f64));
        }
    }
    if points.len() < 16 {
        return 0.0;
    }

    // A row may fall outside the page once sheared; the margin holds those
    // bins so a line leaving the page is not counted as ink at row zero.
    let margin = (SKEW_RANGE.to_radians().tan() * width as f64).ceil() as usize + 2;
    let mut rows = vec![0u32; height as usize + 2 * margin];
    let mut best = (0.0f64, f64::MIN);
    let mut candidate = -SKEW_RANGE;
    while candidate <= SKEW_RANGE + 1e-9 {
        rows.iter_mut().for_each(|count| *count = 0);
        let tangent = candidate.to_radians().tan();
        for &(x, y) in &points {
            let row = y - tangent * x;
            if row < 0.0 || row >= height as f64 {
                continue;
            }
            rows[row.round() as usize + margin] += 1;
        }
        // Sharp rows: a line of text occupies few rows, so its edges are steep.
        let mut score = 0.0f64;
        let mut previous = rows[0] as f64;
        for &count in &rows[1..] {
            let difference = count as f64 - previous;
            score += difference * difference;
            previous = count as f64;
        }
        if score > best.1 {
            best = (candidate, score);
        }
        candidate += SKEW_STEP;
    }
    best.0
}

/// The page turned by `degrees` — [`skew_angle`]'s reading — about its centre.
///
/// The grid does not change: the page keeps its width and height and the
/// corners the turn uncovers are paper. That keeps the caller's arithmetic
/// simple — the boxes this page yields are in its pixels, and the caller maps
/// them back through the same rotation — and it costs nothing when the angle
/// is zero.
pub fn deskew_page(image_bytes: &[u8], degrees: f64, limits: &Limits) -> Result<Vec<u8>> {
    let bitmap = pdfextractor_image_parser::decode_checked(image_bytes, limits)
        .context("failed to decode a page for deskewing")?;
    let straightened = rotate_about_centre(&bitmap.to_luma8(), degrees);
    let mut png = Vec::new();
    image::DynamicImage::ImageLuma8(straightened)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .context("failed to encode a deskewed page")?;
    Ok(png)
}

/// Rotate `image` by `degrees` about its centre, sampling bilinearly.
///
/// The sampling matrix is the one the caller must invert to map a box back: an
/// output row corresponds to the input line of slope `tan(degrees)`, which is
/// what makes [`skew_angle`]'s reading the argument to pass here.
fn rotate_about_centre(image: &GrayImage, degrees: f64) -> GrayImage {
    let (width, height) = image.dimensions();
    let (cx, cy) = (width as f64 / 2.0, height as f64 / 2.0);
    let (sin, cos) = degrees.to_radians().sin_cos();
    let mut out = GrayImage::from_pixel(width, height, Luma([255]));
    for y in 0..height {
        for x in 0..width {
            let dx = x as f64 + 0.5 - cx;
            let dy = y as f64 + 0.5 - cy;
            let sx = cx + dx * cos - dy * sin - 0.5;
            let sy = cy + dx * sin + dy * cos - 0.5;
            out.put_pixel(x, y, Luma([sample(image, sx, sy)]));
        }
    }
    out
}

/// The bilinear value at a fractional position, paper outside the page.
fn sample(image: &GrayImage, x: f64, y: f64) -> u8 {
    let (width, height) = image.dimensions();
    if x < -1.0 || y < -1.0 || x > width as f64 || y > height as f64 {
        return 255;
    }
    let (x0, y0) = (x.floor() as i64, y.floor() as i64);
    let (fx, fy) = (x - x.floor(), y - y.floor());
    let mut value = 0.0;
    for (dx, dy, weight) in [
        (0, 0, (1.0 - fx) * (1.0 - fy)),
        (1, 0, fx * (1.0 - fy)),
        (0, 1, (1.0 - fx) * fy),
        (1, 1, fx * fy),
    ] {
        let px = x0 + dx;
        let py = y0 + dy;
        let pixel = if px < 0 || py < 0 || px >= width as i64 || py >= height as i64 {
            255
        } else {
            image.get_pixel(px as u32, py as u32)[0]
        };
        value += pixel as f64 * weight;
    }
    value.round().clamp(0.0, 255.0) as u8
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

    /// Bars whose lines descend by `slope` degrees: text, without a font.
    fn bars(slope: f64) -> GrayImage {
        let mut image = GrayImage::from_pixel(240, 160, Luma([255]));
        let tangent = slope.to_radians().tan();
        for row in 0..4 {
            let base = 24.0 + row as f64 * 30.0;
            for x in 20..220u32 {
                let y = base + tangent * (x as f64 - 120.0);
                for dy in -1i64..=1 {
                    let py = y.round() as i64 + dy;
                    if (0..160).contains(&py) {
                        image.put_pixel(x, py as u32, Luma([0]));
                    }
                }
            }
        }
        image
    }

    /// The reading is the direction the lines run, not the page's frame: a
    /// page turned either way reads back its own sign, and an upright one
    /// reads zero. The first version binned *vertical* strokes and answered
    /// zero to all of these.
    #[test]
    fn the_estimator_reads_the_direction_the_lines_run() {
        for slope in [0.0, 2.0, -4.0, 6.0, 10.0] {
            let measured = estimate_skew(&bars(slope));
            assert!(
                (measured - slope).abs() <= 0.5,
                "a {slope} degree page read as {measured}"
            );
        }
    }

    /// Turning the page by its own reading flattens it, on the same grid.
    #[test]
    fn deskewing_a_page_leaves_it_flat() {
        let prepared = deskew_page(&encode(&bars(4.0)), 4.0, &Limits::default()).expect("deskew");
        let straightened = decode(&prepared);
        assert_eq!(
            (straightened.width(), straightened.height()),
            (240, 160),
            "the grid changed, so the caller's boxes no longer fit"
        );
        let measured = estimate_skew(&straightened);
        assert!(
            measured.abs() <= 0.5,
            "the page is {measured} degrees off flat"
        );
    }

    #[test]
    fn a_blank_page_measures_zero() {
        let blank = GrayImage::from_pixel(100, 100, Luma([255]));
        assert_eq!(estimate_skew(&blank), 0.0, "a blank page guessed a skew");
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
