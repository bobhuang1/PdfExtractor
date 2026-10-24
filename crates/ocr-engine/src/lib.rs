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
use std::io::Cursor;
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

/// A quarter turn that puts a page upright, applied clockwise.
///
/// A page can be on its side with nothing in the file saying so: a scanner may
/// write the pixels sideways with no PDF `/Rotate`, and an image may carry no
/// EXIF orientation tag at all. Tesseract reads the pixels as stored, so a
/// sideways page comes back as noise unless it is turned first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QuarterTurn {
    /// The pixels are already upright.
    #[default]
    AsStored,
    /// Stored on its left side: turn 90° clockwise.
    Clockwise90,
    /// Upside down: turn 180°.
    Clockwise180,
    /// Stored on its right side: turn 270° clockwise (90° anticlockwise).
    Clockwise270,
}

impl QuarterTurn {
    /// Every turn the detector considers, the stored one first.
    pub const ALL: [QuarterTurn; 4] = [
        QuarterTurn::AsStored,
        QuarterTurn::Clockwise90,
        QuarterTurn::Clockwise180,
        QuarterTurn::Clockwise270,
    ];

    /// The turn clockwise, in degrees.
    pub fn degrees(self) -> i32 {
        match self {
            QuarterTurn::AsStored => 0,
            QuarterTurn::Clockwise90 => 90,
            QuarterTurn::Clockwise180 => 180,
            QuarterTurn::Clockwise270 => 270,
        }
    }

    /// True when turning the page swaps its width and height.
    pub fn swaps_axes(self) -> bool {
        matches!(self, QuarterTurn::Clockwise90 | QuarterTurn::Clockwise270)
    }
}

/// True when this build can actually perform OCR.
pub const OCR_ENABLED: bool = cfg!(feature = "tesseract");

/// Default resolution assumed for images that do not declare one. Tesseract
/// warns and assumes badly below ~70 dpi.
#[cfg(feature = "tesseract")]
const FALLBACK_DPI: i32 = 300;

/// The longest edge, in pixels, of the copy the orientation detector reads.
///
/// A page is recognised in up to four orientations to find its upright one, so
/// the detector works on a reduced copy: at 1000 px an A4 page is still around
/// 120 dpi, which is plenty of detail for Tesseract to have an opinion about,
/// at a fraction of the cost of full-size passes.
#[cfg(feature = "tesseract")]
const DETECT_LONG_EDGE: u32 = 1000;

/// The confidence below which a word is noise rather than reading.
#[cfg(feature = "tesseract")]
const DETECT_WORD_CONFIDENCE: f32 = 55.0;

/// The least a reading must contain, in confidence-weighted characters, before
/// it can be a reason to turn the page at all: about a dozen characters at the
/// confidence above.
#[cfg(feature = "tesseract")]
const DETECT_MIN_SCORE: f64 = 600.0;

/// A reading this strong as stored is a page whose lines are the way text
/// reads, so the two *quarter* turns need not be tried — a sideways page can
/// only produce fragments. This is what keeps an upright scan, nearly every
/// page OCR sees, to two recognitions in the detector rather than four: the
/// stored turn and the half turn, which is always read because an upside-down
/// page still has horizontal baselines (see [`OcrEngine::detect_upright`]).
#[cfg(feature = "tesseract")]
const DETECT_SETTLED_SCORE: f64 = 1500.0;

/// How much better another turn must read than the stored pixels before the
/// page is turned. A page that reads a little worse upright than sideways —
/// sparse text, a noisy scan — is not evidence that it was captured sideways.
#[cfg(feature = "tesseract")]
const DETECT_MARGIN: f64 = 1.35;

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
    // A package's own tessdata becomes Tesseract's whole world once it is
    // pointed at, so it is pointed at only when it carries *every* requested
    // language: a package with just English must not claim `eng+deu`, or the
    // system's German data would be hidden behind an empty lookup.
    let complete = !lang.is_empty()
        && lang
            .split('+')
            .all(|l| tessdata.join(format!("{l}.traineddata")).is_file());
    complete.then_some(tessdata)
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

    /// Choose Tesseract's page segmentation mode, 0–13 — the `PageSegMode` of
    /// `publictypes.h` (3 is the engine's own default; 6 is one block, 7 one
    /// line, 11 sparse text). `leptess` has no typed setter for the mode, but
    /// Tesseract exposes it as a variable and the bindings do.
    pub fn set_psm(&mut self, psm: u8) -> Result<()> {
        #[cfg(feature = "tesseract")]
        {
            if psm > 13 {
                anyhow::bail!("page segmentation mode must be 0–13, got {psm}");
            }
            self.tess
                .set_variable(leptess::Variable::TesseditPagesegMode, &psm.to_string())
                .map_err(|_| anyhow::anyhow!("Tesseract refused the page segmentation mode {psm}"))
        }
        #[cfg(not(feature = "tesseract"))]
        {
            let _ = psm;
            anyhow::bail!("OCR support was not compiled in")
        }
    }

    /// Which way a page has to be turned to be read upright, judged by
    /// Tesseract itself.
    ///
    /// The page is recognised in up to four turns on a reduced copy, and the
    /// turn whose reading holds the most confident text wins.
    /// No extra data is needed: whatever language the engine was built for is
    /// the language the page is judged in.
    ///
    /// The stored pixels keep the page unless another turn reads *clearly*
    /// better (see [`DETECT_MARGIN`]). A page with little text, a blurry scan
    /// and one this cannot decode all answer [`QuarterTurn::AsStored`], so a
    /// page is never turned on a difference that is only noise.
    ///
    /// The half turn is always read with the stored one: upside-down text has
    /// horizontal baselines, so Tesseract reads it as other words rather than
    /// as a turned page, and only a stronger reading the right way round shows
    /// it. The two quarter turns are read only when the stored reading is weak
    /// ([`DETECT_SETTLED_SCORE`]), which is the shape a sideways page makes.
    pub fn detect_upright(&mut self, image_bytes: &[u8]) -> Result<QuarterTurn> {
        #[cfg(feature = "tesseract")]
        {
            let bitmap = image::load_from_memory(image_bytes)
                .context("failed to decode the image whose orientation was to be found")?;
            let sample = downscale_for_detection(&bitmap);

            let stored = self.read_turn(&sample, QuarterTurn::AsStored)?;
            let mut readings = vec![
                (QuarterTurn::AsStored, stored),
                (
                    QuarterTurn::Clockwise180,
                    self.read_turn(&sample, QuarterTurn::Clockwise180)?,
                ),
            ];
            if stored < DETECT_SETTLED_SCORE {
                for turn in [QuarterTurn::Clockwise90, QuarterTurn::Clockwise270] {
                    readings.push((turn, self.read_turn(&sample, turn)?));
                }
            }
            tracing::debug!("orientation readings (turn, score): {readings:?}");
            Ok(choose_upright(&readings))
        }
        #[cfg(not(feature = "tesseract"))]
        {
            let _ = image_bytes;
            anyhow::bail!("OCR support was not compiled in")
        }
    }

    /// Recognise the reduced page in one turn and score what came back.
    #[cfg(feature = "tesseract")]
    fn read_turn(&mut self, sample: &image::GrayImage, turn: QuarterTurn) -> Result<f64> {
        let turned = match turn {
            QuarterTurn::AsStored => sample.clone(),
            QuarterTurn::Clockwise90 => image::imageops::rotate90(sample),
            QuarterTurn::Clockwise180 => image::imageops::rotate180(sample),
            QuarterTurn::Clockwise270 => image::imageops::rotate270(sample),
        };
        let mut png = Vec::new();
        image::DynamicImage::ImageLuma8(turned)
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .context("failed to encode the image for its orientation check")?;
        Ok(reading_score(&self.recognize(&png)?))
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

/// The page reduced to the size the orientation detector reads, as grey: the
/// turn is the question, so colour is only a cost here.
#[cfg(feature = "tesseract")]
fn downscale_for_detection(bitmap: &image::DynamicImage) -> image::GrayImage {
    let gray = bitmap.to_luma8();
    let long = gray.width().max(gray.height());
    if long <= DETECT_LONG_EDGE {
        return gray;
    }
    let scale = DETECT_LONG_EDGE as f64 / long as f64;
    let width = ((gray.width() as f64 * scale).round() as u32).max(1);
    let height = ((gray.height() as f64 * scale).round() as u32).max(1);
    image::imageops::resize(&gray, width, height, image::imageops::FilterType::Triangle)
}

/// How much this reading looks like a page: the characters of each confident
/// word, weighted by that word's confidence. A sideways page read as stored
/// yields few words, or short ones, or words the engine is unsure of — all of
/// which score far below the same page read the right way up.
#[cfg(feature = "tesseract")]
fn reading_score(reading: &OcrResult) -> f64 {
    reading
        .words
        .iter()
        .map(|word| {
            let letters = word.text.chars().filter(|c| c.is_alphanumeric()).count();
            if letters < 2 || word.confidence < DETECT_WORD_CONFIDENCE {
                return 0.0;
            }
            word.confidence as f64 * letters as f64
        })
        .sum()
}

/// Pick the turn a page is read best in — or the stored one, unless the gain
/// is large enough to be the page's orientation rather than noise.
#[cfg(feature = "tesseract")]
fn choose_upright(readings: &[(QuarterTurn, f64)]) -> QuarterTurn {
    let stored = readings
        .iter()
        .find(|(turn, _)| *turn == QuarterTurn::AsStored)
        .map(|(_, score)| *score)
        .unwrap_or(0.0);
    // `>` keeps the earlier turn on a tie, so the stored page wins a draw.
    let (best, score) =
        readings
            .iter()
            .copied()
            .fold((QuarterTurn::AsStored, stored), |best, next| {
                if next.1 > best.1 {
                    next
                } else {
                    best
                }
            });
    if best == QuarterTurn::AsStored || score < DETECT_MIN_SCORE || score < stored * DETECT_MARGIN {
        QuarterTurn::AsStored
    } else {
        best
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

    /// A reading with just these words, at these confidences.
    fn reading(words: &[(&str, f32)]) -> OcrResult {
        OcrResult {
            text: String::new(),
            lines: Vec::new(),
            words: words
                .iter()
                .map(|(text, confidence)| OcrWord {
                    text: text.to_string(),
                    bbox: [0, 0, 1, 1],
                    confidence: *confidence,
                })
                .collect(),
        }
    }

    /// The scores of the same page read in all four turns, upright and
    /// `(turn, words)`.
    fn turns(
        upright: &[(&str, f32)],
        others: &[(QuarterTurn, &[(&str, f32)])],
    ) -> Vec<(QuarterTurn, f64)> {
        let mut scores = vec![(QuarterTurn::AsStored, reading_score(&reading(upright)))];
        for (turn, words) in others {
            scores.push((*turn, reading_score(&reading(words))));
        }
        scores
    }

    #[test]
    fn a_page_is_read_best_in_its_upright_turn() {
        let sideways = reading(&[("|", 91.0), ("I", 88.0), ("-", 95.0)]);
        assert_eq!(
            reading_score(&sideways),
            0.0,
            "a turn that finds no words is no reason to turn the page"
        );

        let scores = turns(
            &[("ACME", 90.0), ("Invoice", 85.0)],
            &[
                (QuarterTurn::Clockwise90, &[("|", 91.0)]),
                (QuarterTurn::Clockwise180, &[("3W3", 70.0)]),
                (QuarterTurn::Clockwise270, &[]),
            ],
        );
        assert_eq!(choose_upright(&scores), QuarterTurn::AsStored);

        // The same page, read better by the quarter turn: the page is sideways.
        let scores = turns(
            &[("|", 80.0)],
            &[
                (
                    QuarterTurn::Clockwise90,
                    &[("ACME", 92.0), ("Invoice", 88.0)],
                ),
                (QuarterTurn::Clockwise180, &[]),
                (QuarterTurn::Clockwise270, &[]),
            ],
        );
        assert_eq!(choose_upright(&scores), QuarterTurn::Clockwise90);
    }

    #[test]
    fn an_upside_down_page_is_turned_round() {
        let scores = turns(
            &[("ıuǝɹɐdS", 52.0)],
            &[
                (QuarterTurn::Clockwise90, &[]),
                (
                    QuarterTurn::Clockwise180,
                    &[("ACME", 91.0), ("Supplies", 89.0)],
                ),
                (QuarterTurn::Clockwise270, &[]),
            ],
        );
        assert_eq!(choose_upright(&scores), QuarterTurn::Clockwise180);
    }

    #[test]
    fn a_close_reading_is_not_a_reason_to_turn_a_page() {
        // Sideways reads a little better by accident; the margin says that is
        // not the page's orientation.
        let scores = turns(
            &[("invoice", 80.0), ("total", 78.0)],
            &[
                (
                    QuarterTurn::Clockwise90,
                    &[("invoice", 84.0), ("total", 80.0)],
                ),
                (QuarterTurn::Clockwise180, &[]),
                (QuarterTurn::Clockwise270, &[]),
            ],
        );
        assert_eq!(choose_upright(&scores), QuarterTurn::AsStored);
    }

    #[test]
    fn a_settled_upright_reading_needs_no_other_turn() {
        // The detector stops after the stored turn when it is already strong;
        // what it is left holding is still the stored page.
        let scores = vec![(
            QuarterTurn::AsStored,
            DETECT_SETTLED_SCORE.max(DETECT_MIN_SCORE) + 1.0,
        )];
        assert_eq!(choose_upright(&scores), QuarterTurn::AsStored);
    }

    #[test]
    fn a_page_with_almost_no_text_is_left_as_stored() {
        // A one-word page reads better turned, but not enough to be evidence:
        // four turns of a nearly empty page must not rotate a document.
        let scores = turns(
            &[("42", 90.0)],
            &[
                (QuarterTurn::Clockwise90, &[("42", 95.0)]),
                (QuarterTurn::Clockwise180, &[]),
                (QuarterTurn::Clockwise270, &[]),
            ],
        );
        assert_eq!(choose_upright(&scores), QuarterTurn::AsStored);
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
        // A combined language list needs every file: the package is
        // Tesseract's whole world once pointed at, so one language is not
        // enough to claim two.
        assert_eq!(tessdata_beside(&dir, "eng+deu"), None);
        std::fs::write(dir.join("tessdata").join("deu.traineddata"), b"").unwrap();
        assert_eq!(tessdata_beside(&dir, "eng+deu"), Some(dir.join("tessdata")));
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
