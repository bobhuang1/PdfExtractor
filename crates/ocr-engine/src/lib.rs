//! OCR via Tesseract.
//!
//! OCR is a required feature. We reuse Tesseract (the mature open-source engine)
//! through the `leptess` bindings rather than reimplementing recognition, and we
//! own the surroundings: image normalisation, word/line grouping, and mapping
//! results into Docling's coordinate space.
//!
//! Word geometry comes from Tesseract's TSV output, whose columns are:
//! `level page_num block_num par_num line_num word_num left top width height conf text`
//! with `level == 5` marking word rows.

#[cfg(feature = "tesseract")]
use anyhow::Context;
use anyhow::Result;

/// A recognised word with a pixel bounding box `[left, top, right, bottom]`.
#[derive(Debug, Clone, PartialEq)]
pub struct OcrWord {
    pub text: String,
    pub bbox: [i32; 4],
    pub confidence: f32,
}

/// A recognised line: its words joined by spaces, plus the union box.
#[derive(Debug, Clone, PartialEq)]
pub struct OcrLine {
    pub text: String,
    pub bbox: [i32; 4],
    pub confidence: f32,
}

/// Everything OCR produced for one image.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OcrResult {
    /// Full text, as Tesseract laid it out.
    pub text: String,
    pub lines: Vec<OcrLine>,
    pub words: Vec<OcrWord>,
}

/// True when this build can actually perform OCR.
pub const OCR_ENABLED: bool = cfg!(feature = "tesseract");

/// Default resolution assumed for images that do not declare one. Tesseract
/// warns and assumes badly below ~70 dpi.
#[cfg(feature = "tesseract")]
const FALLBACK_DPI: i32 = 300;

/// An OCR engine bound to one language.
///
/// `LepTess` is not `Sync`, so each worker owns its own engine. Construction is
/// comparatively expensive, so build one per conversion, not per page.
pub struct OcrEngine {
    #[cfg(feature = "tesseract")]
    tess: leptess::LepTess,
}

impl OcrEngine {
    /// Create an engine for `lang` (e.g. `"eng"`).
    pub fn new(lang: &str) -> Result<Self> {
        #[cfg(feature = "tesseract")]
        {
            let tess = leptess::LepTess::new(None, lang).with_context(|| {
                format!(
                    "failed to initialise Tesseract for language '{lang}'; is tessdata installed?"
                )
            })?;
            Ok(Self { tess })
        }
        #[cfg(not(feature = "tesseract"))]
        {
            let _ = lang;
            anyhow::bail!("OCR support was not compiled in; rebuild with the `tesseract` feature")
        }
    }

    /// Recognise text in an encoded image (PNG, JPEG, TIFF, BMP...).
    pub fn recognize(&mut self, image_bytes: &[u8]) -> Result<OcrResult> {
        #[cfg(feature = "tesseract")]
        {
            self.tess
                .set_image_from_mem(image_bytes)
                .context("failed to load image into Tesseract (unsupported encoding?)")?;
            // Supply a sane dpi when the image does not carry one.
            self.tess.set_fallback_source_resolution(FALLBACK_DPI);

            let text = self
                .tess
                .get_utf8_text()
                .context("Tesseract failed to produce text")?;
            let tsv = self
                .tess
                .get_tsv_text(0)
                .context("Tesseract failed to produce TSV output")?;

            let mut result = parse_tsv(&tsv);
            result.text = text;
            Ok(result)
        }
        #[cfg(not(feature = "tesseract"))]
        {
            let _ = image_bytes;
            anyhow::bail!("OCR support was not compiled in")
        }
    }
}

/// Parse Tesseract's TSV into words grouped into lines.
#[cfg(feature = "tesseract")]
fn parse_tsv(tsv: &str) -> OcrResult {
    let mut words: Vec<OcrWord> = Vec::new();
    let mut lines: Vec<OcrLine> = Vec::new();
    // (block, paragraph, line) identifying the current line.
    let mut current_key: Option<(i32, i32, i32)> = None;

    for row in tsv.lines().skip(1) {
        // The text column may itself contain tabs, so keep the tail intact.
        let cols: Vec<&str> = row.splitn(12, '\t').collect();
        if cols.len() < 12 {
            continue;
        }
        // Only word-level rows carry the geometry we want.
        if cols[0].trim() != "5" {
            continue;
        }
        let text = cols[11].trim();
        if text.is_empty() {
            continue;
        }

        let key = (
            cols[2].parse().unwrap_or(0),
            cols[3].parse().unwrap_or(0),
            cols[4].parse().unwrap_or(0),
        );
        let left: i32 = cols[6].parse().unwrap_or(0);
        let top: i32 = cols[7].parse().unwrap_or(0);
        let width: i32 = cols[8].parse().unwrap_or(0);
        let height: i32 = cols[9].parse().unwrap_or(0);
        let confidence: f32 = cols[10].parse().unwrap_or(-1.0);
        let bbox = [left, top, left + width, top + height];

        words.push(OcrWord {
            text: text.to_string(),
            bbox,
            confidence,
        });

        match current_key {
            Some(k) if k == key => {
                if let Some(line) = lines.last_mut() {
                    line.text.push(' ');
                    line.text.push_str(text);
                    line.bbox = union(line.bbox, bbox);
                    line.confidence = line.confidence.min(confidence);
                }
            }
            _ => {
                lines.push(OcrLine {
                    text: text.to_string(),
                    bbox,
                    confidence,
                });
                current_key = Some(key);
            }
        }
    }

    OcrResult {
        text: String::new(),
        lines,
        words,
    }
}

#[cfg(feature = "tesseract")]
fn union(a: [i32; 4], b: [i32; 4]) -> [i32; 4] {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}

#[cfg(all(test, feature = "tesseract"))]
mod tests {
    use super::*;

    const TSV: &str = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
1\t1\t0\t0\t0\t0\t0\t0\t600\t200\t-1\t\n\
5\t1\t1\t1\t1\t1\t10\t10\t40\t12\t96.5\tHello\n\
5\t1\t1\t1\t1\t2\t55\t10\t45\t12\t95.0\tWorld\n\
5\t1\t1\t1\t2\t1\t10\t40\t30\t12\t90.0\tAgain\n";

    #[test]
    fn parses_words_and_groups_lines() {
        let r = parse_tsv(TSV);
        assert_eq!(r.words.len(), 3);
        assert_eq!(r.words[0].text, "Hello");
        assert_eq!(r.words[0].bbox, [10, 10, 50, 22]);
        assert_eq!(r.lines.len(), 2, "two visual lines");
        assert_eq!(r.lines[0].text, "Hello World");
        // The union box spans both words on line one.
        assert_eq!(r.lines[0].bbox, [10, 10, 100, 22]);
        assert_eq!(r.lines[1].text, "Again");
    }

    #[test]
    fn ignores_non_word_rows_and_blanks() {
        let r = parse_tsv(TSV);
        assert!(r.words.iter().all(|w| !w.text.trim().is_empty()));
    }
}
