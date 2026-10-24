//! The built-in font the text records are drawn with.
//!
//! A metafile does not carry glyphs. A text record names a font — `Liberation
//! Serif`, `Tahoma`, `System` — and the *system* supplies the outlines when the
//! drawing is played back. A document extractor has no business fetching fonts
//! from the machine it happens to run on: the picture would change with the
//! machine, and on a server there may be no fonts at all. So the characters are
//! drawn in a 5×7 font defined here, which is a *substitute*: where a record
//! says a glyph belongs, how tall it is and what colour are the file's; the
//! shape of the glyph is this crate's. It is coarse at large sizes, and that is
//! the honest cost of not depending on the host's fonts.
//!
//! Only the printable ASCII range is drawn. A character outside it advances the
//! pen without drawing, rather than being replaced by a placeholder that a
//! reader might mistake for the document's own text.

use crate::paint::{pt, Canvas, Color, FillRule, Point};

/// Columns in a glyph's cell.
const GLYPH_COLS: usize = 5;
/// Rows in a glyph's cell: five for the body, two for descenders.
const GLYPH_ROWS: usize = 7;
/// Columns the pen moves per character, the glyph plus its gap.
const ADVANCE: usize = 6;
/// The row the baseline sits on: rows 5 and 6 hang below it.
const BASELINE_ROW: f64 = 5.0;

/// Where the reference point of a text record sits in the string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Horizontal {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vertical {
    Top,
    Baseline,
    Bottom,
}

/// The text-alignment values a metafile sets, as one record's worth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextAlign {
    pub horizontal: Horizontal,
    pub vertical: Vertical,
}

impl Default for TextAlign {
    fn default() -> Self {
        TextAlign {
            horizontal: Horizontal::Left,
            vertical: Vertical::Top,
        }
    }
}

/// Draw `text` with its reference point at `origin`, `cell_height` canvas pixels
/// tall, turned by `escapement_tenths` of a degree about that point.
///
/// `background` is the colour behind the text when the device context is in its
/// opaque mode — the box GDI fills in before drawing the glyphs — or `None` when
/// it is transparent.
///
/// Clipping, colour and the rasterising are the canvas's; this only decides
/// where each part of each glyph goes.
#[allow(clippy::too_many_arguments)]
pub fn draw_text(
    canvas: &mut Canvas,
    text: &str,
    origin: Point,
    cell_height: f64,
    align: TextAlign,
    escapement_tenths: f64,
    color: Color,
    background: Option<Color>,
) {
    if text.is_empty() || !cell_height.is_finite() || cell_height <= 0.0 {
        return;
    }
    let dot = cell_height / GLYPH_ROWS as f64;
    if !dot.is_finite() || dot <= 0.0 {
        return;
    }
    let advance = dot * ADVANCE as f64;
    let characters = text.chars().count() as f64;
    let start_x = match align.horizontal {
        Horizontal::Left => 0.0,
        Horizontal::Center => -characters * advance / 2.0,
        Horizontal::Right => -characters * advance,
    };
    let start_y = match align.vertical {
        Vertical::Top => 0.0,
        Vertical::Baseline => -dot * BASELINE_ROW,
        Vertical::Bottom => -dot * GLYPH_ROWS as f64,
    };
    let (sin, cos) = (escapement_tenths * 0.1).to_radians().sin_cos();
    let place = |local: Point| Point {
        x: origin.x + local.x * cos - local.y * sin,
        y: origin.y + local.x * sin + local.y * cos,
    };

    let mut pen = start_x;
    for character in text.chars() {
        if let Some(color) = background {
            // The opaque background is the whole cell the pen moves over.
            let quad = vec![
                place(pt(pen, start_y)),
                place(pt(pen + advance, start_y)),
                place(pt(pen + advance, start_y + dot * GLYPH_ROWS as f64)),
                place(pt(pen, start_y + dot * GLYPH_ROWS as f64)),
            ];
            canvas.fill_polygons(&[quad], FillRule::Alternate, color);
        }
        if let Some(glyph) = glyph(character) {
            for (row, bits) in glyph.iter().enumerate() {
                let y = start_y + row as f64 * dot;
                // Fill each run of set dots in the row as one rectangle.
                let mut column = 0usize;
                while column < GLYPH_COLS {
                    if bits & (1 << (GLYPH_COLS - 1 - column)) == 0 {
                        column += 1;
                        continue;
                    }
                    let run_start = column;
                    while column < GLYPH_COLS && bits & (1 << (GLYPH_COLS - 1 - column)) != 0 {
                        column += 1;
                    }
                    let x = pen + run_start as f64 * dot;
                    let right = pen + column as f64 * dot;
                    let quad = vec![
                        place(pt(x, y)),
                        place(pt(right, y)),
                        place(pt(right, y + dot)),
                        place(pt(x, y + dot)),
                    ];
                    canvas.fill_polygons(&[quad], FillRule::Alternate, color);
                }
            }
        }
        pen += advance;
    }
}

/// How wide `text` is when drawn `cell_height` pixels tall.
///
/// Every character takes one cell and its gap, which is the pen movement
/// [`draw_text`] makes; a caller that has to fit a string into a box decides the
/// height it asks for from this. Nothing outside printable ASCII is drawn, but
/// it still advances the pen, so it is counted here too.
pub fn text_width(text: &str, cell_height: f64) -> f64 {
    if !cell_height.is_finite() || cell_height <= 0.0 {
        return 0.0;
    }
    text.chars().count() as f64 * (cell_height / GLYPH_ROWS as f64) * ADVANCE as f64
}

/// The dot pattern of a character, or `None` when the font has no glyph for it.
fn glyph(character: char) -> Option<&'static [u8; GLYPH_ROWS]> {
    let code = character as u32;
    if !(32..127).contains(&code) {
        return None;
    }
    GLYPHS.get((code - 32) as usize)
}

/// A 5×7 font for printable ASCII, one row per line, the leftmost column in bit
/// 4 and the top row first — so each glyph can be read off the source.
#[rustfmt::skip]
static GLYPHS: [[u8; GLYPH_ROWS]; 95] = [
    [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000], // (space)
    [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00100], // !
    [0b01010, 0b01010, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000], // "
    [0b01010, 0b01010, 0b11111, 0b01010, 0b11111, 0b01010, 0b01010], // #
    [0b00100, 0b01111, 0b10100, 0b01110, 0b00101, 0b11110, 0b00100], // $
    [0b11000, 0b11001, 0b00010, 0b00100, 0b01000, 0b10011, 0b00011], // %
    [0b01100, 0b10010, 0b10100, 0b01000, 0b10101, 0b10010, 0b01101], // &
    [0b00100, 0b00100, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000], // '
    [0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010], // (
    [0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000], // )
    [0b00000, 0b00100, 0b10101, 0b01110, 0b10101, 0b00100, 0b00000], // *
    [0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000], // +
    [0b00000, 0b00000, 0b00000, 0b00000, 0b00110, 0b00100, 0b01000], // ,
    [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000], // -
    [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00110, 0b00110], // .
    [0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000], // /
    [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110], // 0
    [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110], // 1
    [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111], // 2
    [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110], // 3
    [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010], // 4
    [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110], // 5
    [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110], // 6
    [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000], // 7
    [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110], // 8
    [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100], // 9
    [0b00000, 0b00110, 0b00110, 0b00000, 0b00110, 0b00110, 0b00000], // :
    [0b00000, 0b00110, 0b00110, 0b00000, 0b00110, 0b00100, 0b01000], // ;
    [0b00010, 0b00100, 0b01000, 0b10000, 0b01000, 0b00100, 0b00010], // <
    [0b00000, 0b00000, 0b11111, 0b00000, 0b11111, 0b00000, 0b00000], // =
    [0b01000, 0b00100, 0b00010, 0b00001, 0b00010, 0b00100, 0b01000], // >
    [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b00000, 0b00100], // ?
    [0b01110, 0b10001, 0b00001, 0b01101, 0b10101, 0b10101, 0b01110], // @
    [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001], // A
    [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110], // B
    [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110], // C
    [0b11100, 0b10010, 0b10001, 0b10001, 0b10001, 0b10010, 0b11100], // D
    [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111], // E
    [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000], // F
    [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111], // G
    [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001], // H
    [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110], // I
    [0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100], // J
    [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001], // K
    [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111], // L
    [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001], // M
    [0b10001, 0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001], // N
    [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110], // O
    [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000], // P
    [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101], // Q
    [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001], // R
    [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110], // S
    [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100], // T
    [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110], // U
    [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100], // V
    [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001], // W
    [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001], // X
    [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100], // Y
    [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111], // Z
    [0b01110, 0b01000, 0b01000, 0b01000, 0b01000, 0b01000, 0b01110], // [
    [0b10000, 0b01000, 0b01000, 0b00100, 0b00010, 0b00010, 0b00001], // backslash
    [0b01110, 0b00010, 0b00010, 0b00010, 0b00010, 0b00010, 0b01110], // ]
    [0b00100, 0b01010, 0b10001, 0b00000, 0b00000, 0b00000, 0b00000], // ^
    [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b11111], // _
    [0b01000, 0b00100, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000], // `
    [0b00000, 0b00000, 0b01110, 0b00001, 0b01111, 0b10001, 0b01111], // a
    [0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b10001, 0b11110], // b
    [0b00000, 0b00000, 0b01111, 0b10000, 0b10000, 0b10000, 0b01111], // c
    [0b00001, 0b00001, 0b01111, 0b10001, 0b10001, 0b10001, 0b01111], // d
    [0b00000, 0b00000, 0b01110, 0b10001, 0b11111, 0b10000, 0b01110], // e
    [0b00110, 0b01001, 0b01000, 0b11100, 0b01000, 0b01000, 0b01000], // f
    [0b00000, 0b01111, 0b10001, 0b10001, 0b01111, 0b00001, 0b01110], // g
    [0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b10001, 0b10001], // h
    [0b00100, 0b00000, 0b01100, 0b00100, 0b00100, 0b00100, 0b01110], // i
    [0b00010, 0b00000, 0b00110, 0b00010, 0b00010, 0b10010, 0b01100], // j
    [0b10000, 0b10000, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010], // k
    [0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110], // l
    [0b00000, 0b00000, 0b11010, 0b10101, 0b10101, 0b10101, 0b10101], // m
    [0b00000, 0b00000, 0b11110, 0b10001, 0b10001, 0b10001, 0b10001], // n
    [0b00000, 0b00000, 0b01110, 0b10001, 0b10001, 0b10001, 0b01110], // o
    [0b00000, 0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000], // p
    [0b00000, 0b01111, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001], // q
    [0b00000, 0b00000, 0b10110, 0b11001, 0b10000, 0b10000, 0b10000], // r
    [0b00000, 0b00000, 0b01111, 0b10000, 0b01110, 0b00001, 0b11110], // s
    [0b01000, 0b01000, 0b11100, 0b01000, 0b01000, 0b01001, 0b00110], // t
    [0b00000, 0b00000, 0b10001, 0b10001, 0b10001, 0b10011, 0b01101], // u
    [0b00000, 0b00000, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100], // v
    [0b00000, 0b00000, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010], // w
    [0b00000, 0b00000, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001], // x
    [0b00000, 0b10001, 0b10001, 0b10001, 0b01111, 0b00001, 0b01110], // y
    [0b00000, 0b00000, 0b11111, 0b00010, 0b00100, 0b01000, 0b11111], // z
    [0b00010, 0b00100, 0b00100, 0b01000, 0b00100, 0b00100, 0b00010], // {
    [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100], // |
    [0b01000, 0b00100, 0b00100, 0b00010, 0b00100, 0b00100, 0b01000], // }
    [0b00000, 0b00000, 0b01001, 0b10110, 0b00000, 0b00000, 0b00000], // ~
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paint::{Canvas, Color};

    /// Every glyph must put ink inside its own cell: a row of dots that is
    /// empty, or a dot outside the five columns, would render as a wrong
    /// character and be invisible to a test that only looked at the text.
    #[test]
    fn every_glyph_fits_its_cell_and_draws_something() {
        for code in 32u32..127 {
            let character = char::from_u32(code).unwrap();
            let glyph = glyph(character).expect("a glyph for every printable character");
            for row in glyph {
                assert_eq!(row & 0b1110_0000, 0, "{character:?}: dots outside the cell");
            }
            if character == ' ' {
                assert!(glyph.iter().all(|row| *row == 0), "space is blank");
                continue;
            }
            assert!(
                glyph.iter().any(|row| *row != 0),
                "{character:?} draws nothing"
            );
        }
    }

    /// Text is laid out left to right and one line tall.
    #[test]
    fn text_is_laid_out_in_a_row() {
        let mut canvas = Canvas::new(60, 20);
        draw_text(
            &mut canvas,
            "ab",
            pt(0.0, 0.0),
            14.0,
            TextAlign::default(),
            0.0,
            Color::BLACK,
            None,
        );
        let ink = |x: i32, y: i32| -> bool {
            let idx = ((y as usize) * 60 + x as usize) * 4;
            canvas.data()[idx] != 255
                || canvas.data()[idx + 1] != 255
                || canvas.data()[idx + 2] != 255
        };
        // Each dot is two pixels across (a 14-pixel cell over seven rows), and
        // the pen advances twelve: 'a' covers x 0..10, 'b' 12..22.
        assert!(ink(2, 9), "no ink in the first character");
        assert!(ink(13, 1), "no ink in the second character");
        assert!(!ink(30, 9), "ink outside both characters");
        assert_eq!(canvas.height(), 20);
    }
}
