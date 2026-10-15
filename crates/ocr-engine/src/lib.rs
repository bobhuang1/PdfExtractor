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
#[cfg(feature = "tesseract")]
use std::path::{Path, PathBuf};

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

/// Where Tesseract should find its data, when the package ships it.
///
/// A released package — the command line's zip, the installed window's bundled
/// copy — carries `tessdata/<lang>.traineddata` beside the executable, so OCR
/// runs on a machine with no Tesseract install of its own (Windows has none to
/// find, and neither macOS nor a minimal container). `None` keeps Tesseract's
/// own default unchanged: its compiled-in data directory, or `TESSDATA_PREFIX`
/// when that is set.
#[cfg(feature = "tesseract")]
fn shipped_datapath(lang: &str) -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let tessdata = tessdata_beside(exe.parent()?, lang)?;
    Some(tessdata.to_string_lossy().into_owned())
}

/// `<beside>/tessdata/<lang>.traineddata`, when the package carries it.
#[cfg(feature = "tesseract")]
fn tessdata_beside(beside: &Path, lang: &str) -> Option<PathBuf> {
    let tessdata = beside.join("tessdata");
    tessdata
        .join(format!("{lang}.traineddata"))
        .is_file()
        .then_some(tessdata)
}

impl OcrEngine {
    /// Create an engine for `lang` (e.g. `"eng"`).
    pub fn new(lang: &str) -> Result<Self> {
        #[cfg(feature = "tesseract")]
        {
            // A package that carries its own tessdata points Tesseract at it;
            // anything else leaves Tesseract to its system default.
            let shipped = shipped_datapath(lang);
            let tess = leptess::LepTess::new(shipped.as_deref(), lang).with_context(|| {
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

    /// A unique scratch directory under the system temp.
    fn scratch(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "pdfextractor-ocr-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// English data from the machine running the test, when it has any.
    fn system_eng() -> Option<PathBuf> {
        let mut candidates = Vec::new();
        if let Ok(prefix) = std::env::var("TESSDATA_PREFIX") {
            let prefix = PathBuf::from(prefix);
            candidates.push(prefix.join("tessdata").join("eng.traineddata"));
            candidates.push(prefix.join("eng.traineddata"));
        }
        candidates.extend([
            PathBuf::from("/usr/share/tesseract-ocr/5/tessdata/eng.traineddata"),
            PathBuf::from("/usr/share/tesseract-ocr/4.00/tessdata/eng.traineddata"),
            PathBuf::from("/opt/homebrew/share/tessdata/eng.traineddata"),
            PathBuf::from("/usr/local/share/tessdata/eng.traineddata"),
        ]);
        candidates.into_iter().find(|p| p.is_file())
    }

    #[test]
    fn a_tessdata_dir_beside_the_binary_is_recognised() {
        let dir = scratch("layout");
        // Nothing beside the binary, nothing to point Tesseract at: the system
        // default stays in force, exactly as before this existed.
        assert_eq!(tessdata_beside(&dir, "eng"), None);

        std::fs::create_dir_all(dir.join("tessdata")).unwrap();
        std::fs::write(dir.join("tessdata").join("eng.traineddata"), b"").unwrap();
        assert_eq!(
            tessdata_beside(&dir, "eng"),
            Some(dir.join("tessdata")),
            "a package's own tessdata directory is recognised"
        );
        // The language must match: other languages are not ours to provide.
        assert_eq!(tessdata_beside(&dir, "fra"), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_package_laid_out_this_way_can_start_ocr() {
        let Some(eng) = system_eng() else {
            eprintln!("no eng.traineddata on this machine; layout test only");
            return;
        };
        let dir = scratch("shipped");
        std::fs::create_dir_all(dir.join("tessdata")).unwrap();
        std::fs::copy(&eng, dir.join("tessdata").join("eng.traineddata")).unwrap();

        let datapath = tessdata_beside(&dir, "eng").expect("the layout is recognised");
        leptess::LepTess::new(Some(datapath.to_str().unwrap()), "eng")
            .expect("Tesseract starts on the package's own tessdata alone");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_datapath_does_not_silently_read_the_system_copy() {
        // The test above proves nothing if Tesseract falls back to its
        // compiled-in data directory whenever the given one comes up empty;
        // on a machine that has system data, this is what rules that out.
        let dir = scratch("empty");
        let started = leptess::LepTess::new(Some(dir.to_str().unwrap()), "eng");
        assert!(
            started.is_err(),
            "an empty datapath started anyway, so Tesseract consulted data outside it"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
