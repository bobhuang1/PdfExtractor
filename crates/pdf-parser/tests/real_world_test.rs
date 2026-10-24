//! Regression tests against real documents produced by other tools.
//!
//! Every assertion here corresponds to a bug that hand-written fixtures did not
//! expose. See `test-data/real/README.md` for provenance.

use pdfextractor_pdf_parser::PdfDocument;
use std::path::PathBuf;

fn real(name: &str) -> Vec<u8> {
    let path = PathBuf::from("../../test-data/real").join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("missing corpus file {}: {e}", path.display()))
}

fn all_text(bytes: &[u8]) -> String {
    let doc = PdfDocument::parse(bytes).expect("parse");
    doc.extract()
        .into_iter()
        .flat_map(|p| p.spans.into_iter().map(|s| s.text))
        .collect::<Vec<_>>()
        .join("\n")
}

fn control_chars(text: &str) -> usize {
    text.chars()
        .filter(|c| (*c as u32) < 32 && *c != '\n' && *c != '\t')
        .count()
}

/// `irs_f1040.pdf` is linearized: it contains an `%%EOF` *mid-file* after the
/// first-page cross-reference section, and hides its page tree inside
/// `/ObjStm` object streams. Stopping at the first `%%EOF` found 2 objects.
#[test]
fn linearized_pdf_with_object_streams_yields_pages() {
    let data = real("irs_f1040.pdf");
    let doc = PdfDocument::parse(&data).expect("parse");

    assert!(
        doc.objects.len() > 100,
        "expected many objects, found {}",
        doc.objects.len()
    );

    let pages = doc.extract();
    assert!(!pages.is_empty(), "page tree lives in an object stream");
    assert!(
        pages.iter().any(|p| !p.spans.is_empty()),
        "expected extractable text"
    );
}

/// WinAnsi bytes above 0x7F must be decoded as cp1252, not emitted raw.
#[test]
fn decodes_winansi_punctuation() {
    let text = all_text(&real("irs_f1040.pdf"));
    assert!(
        text.contains("Department of the Treasury\u{2014}Internal Revenue"),
        "em dash (cp1252 0x97) was not decoded"
    );
    assert_eq!(
        control_chars(&text),
        0,
        "control characters leaked into text"
    );
}

/// `/Differences` maps a non-printing code to a ligature glyph name.
#[test]
fn decodes_ligature_from_encoding_differences() {
    let text = all_text(&real("arxiv_attention.pdf"));
    assert!(
        text.contains("tables and figures"),
        "the fi ligature was not decoded"
    );
}

/// A TeX symbol font declares no `/Encoding`: its codes are the font program's
/// own, and as Latin-1 they are nothing (`cmsy` keeps `\u{2217}` at 3, which is
/// a control character) or the wrong letter (`\u{2020}`/`\u{2021}` at 121/122,
/// which are `y`/`z`). The encodings are read from the embedded program.
#[test]
fn recovers_symbols_from_the_font_programs_own_encoding() {
    let gan = all_text(&real("math_gan.pdf"));
    for (symbol, what) in [
        ('\u{2217}', "the ∗ markers"),
        ('\u{2020}', "the † markers"),
        ('\u{2021}', "the ‡ markers"),
    ] {
        assert!(gan.contains(symbol), "math_gan: {what} were not recovered");
    }
    // The author lines the markers introduce are no longer `y`/`z`. The marker is
    // its own span — the page draws it in the symbol font — so the spans are joined
    // without the whitespace between them for this check.
    let joined: String = gan.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(
        joined.contains("\u{2020}SherjilOzairisvisiting"),
        "math_gan: the † note lost its marker"
    );
    assert!(
        !joined.contains("ySherjilOzairisvisiting"),
        "math_gan: the † is still a `y`"
    );

    let seq: String = all_text(&real("math_seq2seq.pdf"))
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    assert!(
        seq.contains("\u{2217}CIFARSeniorFellow"),
        "math_seq2seq: the ∗CIFAR note lost its marker"
    );
}

/// Identity-H (Type0) fonts encode every character in two bytes.
#[test]
fn decodes_identity_h_composite_font() {
    let text = all_text(&real("basicapi.pdf"));
    assert!(
        text.contains("Chapter"),
        "2-byte codes were not decoded, got: {:?}",
        &text[..text.len().min(80)]
    );
}

/// Producers separate words by repositioning, not by space characters, so the
/// extractor must infer gaps from `TJ` kerning and `Td` advances.
#[test]
fn infers_word_spaces_from_positioning() {
    let text = all_text(&real("arxiv_attention.pdf"));
    assert!(
        text.contains("Provided proper attribution is provided"),
        "words were concatenated"
    );
    assert!(
        text.contains("Attention Is All You Need"),
        "title was not recovered intact"
    );
}

/// `dummy.pdf` draws every run as its own `Td` — and each jump lands within
/// half a point of where the glyph advances already stopped, so the moves are
/// repositioning, not gaps. Reading every same-line `Td` as a word break split
/// the words apart (`Dumm y  PDF  fi le`): the page carries its own space
/// characters, and they are the spaces a reader sees.
#[test]
fn a_td_that_lands_on_the_advance_does_not_split_the_word() {
    let text = all_text(&real("dummy.pdf"));
    assert_eq!(
        text.trim(),
        "Dummy PDF file",
        "run repositioning was read as a word gap: {text:?}"
    );
}

/// Figures in these papers are drawn with vector paths: there is no image
/// XObject anywhere, so detection has to come from painted paths.
#[test]
fn finds_vector_figures_with_no_embedded_image() {
    for name in ["math_batchnorm.pdf", "math_seq2seq.pdf", "tracemonkey.pdf"] {
        let doc = PdfDocument::parse(&real(name)).expect("parse");
        let pages = doc.extract();
        let figures: Vec<_> = pages.iter().flat_map(|p| p.figures.iter()).collect();
        assert!(!figures.is_empty(), "{name}: no figures detected");
        // Every reported figure must be a plausible figure, not page furniture.
        for page in &pages {
            for f in &page.figures {
                let w = f[2] - f[0];
                let h = f[3] - f[1];
                let frac = (w * h) / (page.width * page.height);
                assert!(
                    frac < 0.5,
                    "{name}: figure covers {:.0}% of the page",
                    frac * 100.0
                );
            }
        }
    }
}

/// A form's own border and shading rectangles cover most of the page; they must
/// not be reported as pictures.
#[test]
fn form_backgrounds_are_not_reported_as_figures() {
    let doc = PdfDocument::parse(&real("irs_f1040.pdf")).expect("parse");
    let pages = doc.extract();
    for page in &pages {
        assert!(
            page.figures.is_empty(),
            "page {}: background reported as a figure: {:?}",
            page.page_no,
            page.figures
        );
    }
}

/// The placement box of a raster image comes from the CTM at its `Do` operator,
/// so it must land inside the page it was drawn on.
#[test]
fn placed_images_get_a_box_inside_their_page() {
    let doc = PdfDocument::parse(&real("math_gan.pdf")).expect("parse");
    let pages = doc.extract();
    let mut seen = 0usize;
    for page in &pages {
        for placed in &page.placed_images {
            seen += 1;
            let [l, t, r, b] = placed.bbox;
            assert!(r > l && b > t, "degenerate box {l},{t},{r},{b}");
            assert!(
                l >= -1.0 && t >= -1.0 && r <= page.width + 1.0 && b <= page.height + 1.0,
                "box {l},{t},{r},{b} escapes page {}x{}",
                page.width,
                page.height
            );
            assert!(placed.image.width > 0 && placed.image.height > 0);
        }
    }
    assert!(seen > 0, "no placed images found in a figure-heavy paper");
}

#[test]
fn extracts_a_full_paper() {
    let doc = PdfDocument::parse(&real("tracemonkey.pdf")).expect("parse");
    let pages = doc.extract();
    assert!(
        pages.len() >= 10,
        "expected the full paper, got {}",
        pages.len()
    );

    let text = pages
        .iter()
        .flat_map(|p| p.spans.iter().map(|s| s.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Trace-based Just-in-Time Type Specialization"));
    assert!(
        text.len() > 50_000,
        "expected substantial text, got {}",
        text.len()
    );
}
