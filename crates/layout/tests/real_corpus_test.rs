//! Layout analysis over the real corpus.
//!
//! The property that matters most is negative-space: grouping runs into blocks
//! must not lose, duplicate, reorder or invent text, and must not drop a
//! picture. Everything else (which label a block gets) is a judgement call that
//! these tests only spot-check.

use pdfextractor_layout::{
    analyze_page, mark_footnotes, mark_furniture, Element, PageElements, Span, FURNITURE_BAND,
};
use pdfextractor_pdf_parser::PdfDocument;
use std::collections::BTreeSet;
use std::path::PathBuf;

fn corpus() -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data/real");
    let mut out: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("the real corpus should be present")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")))
        .collect();
    out.sort();
    assert!(
        out.len() >= 8,
        "expected the downloaded corpus, found {}",
        out.len()
    );
    out
}

/// The characters that must survive, sorted so the comparison is about content
/// rather than order: layout reorders runs on purpose, may change spacing, and
/// may join a hyphenated line break, but it may not lose, duplicate or invent a
/// letter or digit.
fn fingerprint(text: &str) -> Vec<char> {
    let mut chars: Vec<char> = text
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    chars.sort_unstable();
    chars
}

struct Page {
    no: i32,
    spans: Vec<Span>,
    size: [f64; 2],
    /// The page's horizontal rules, as the parser published them.
    rules: Vec<[f64; 4]>,
}

fn pages_of(bytes: &[u8]) -> Vec<Page> {
    let parsed = PdfDocument::parse(bytes).expect("corpus PDFs should parse");
    parsed
        .extract()
        .into_iter()
        .map(|page| Page {
            no: page.page_no,
            spans: page
                .spans
                .iter()
                .map(|s| Span {
                    text: s.text.clone(),
                    bbox: s.bbox,
                    font_size: s.font_size,
                    rotated: s.rotated,
                })
                .collect(),
            size: [page.width, page.height],
            rules: page.rules,
        })
        .collect()
}

/// Every character the document would contain.
fn text_of(elements: &[Element]) -> Vec<char> {
    let mut out = Vec::new();
    for element in elements {
        match element {
            Element::Text { text, marker, .. } => {
                if let Some(marker) = marker {
                    out.extend(fingerprint(marker));
                }
                out.extend(fingerprint(text));
            }
            Element::Table { rows, .. } => {
                for row in rows {
                    for cell in row {
                        out.extend(fingerprint(cell));
                    }
                }
            }
            Element::Picture { .. } => {}
        }
    }
    out.sort_unstable();
    out
}

#[test]
fn layout_never_loses_or_duplicates_a_character() {
    for path in corpus() {
        let bytes = std::fs::read(&path).expect("readable");
        let name = path
            .file_name()
            .expect("a name")
            .to_string_lossy()
            .to_string();
        for page in pages_of(&bytes) {
            let elements = analyze_page(&page.spans, page.size, &[], &page.rules, page.no);
            let mut before: Vec<char> = Vec::new();
            for span in &page.spans {
                before.extend(fingerprint(&span.text));
            }
            before.sort_unstable();
            let after = text_of(&elements);
            assert_eq!(before, after, "{name} page {} changed the text", page.no);
        }
    }
}

#[test]
fn every_picture_is_placed_exactly_once() {
    let pictures = [[10.0, 20.0, 200.0, 100.0], [300.0, 400.0, 500.0, 600.0]];
    for path in corpus() {
        let bytes = std::fs::read(&path).expect("readable");
        let name = path
            .file_name()
            .expect("a name")
            .to_string_lossy()
            .to_string();
        for page in pages_of(&bytes) {
            // A picture's own boxes have nothing to do with where it should land,
            // so the same two boxes are offered on every page.
            let elements = analyze_page(&page.spans, page.size, &pictures, &page.rules, page.no);
            let mut seen = Vec::new();
            for element in &elements {
                if let Element::Picture { index, .. } = element {
                    seen.push(*index);
                }
            }
            seen.sort_unstable();
            assert_eq!(
                seen,
                vec![0, 1],
                "{name} page {} lost or repeated a picture",
                page.no
            );
        }
    }
}

#[test]
fn no_box_escapes_its_page_or_collapses() {
    for path in corpus() {
        let bytes = std::fs::read(&path).expect("readable");
        let name = path
            .file_name()
            .expect("a name")
            .to_string_lossy()
            .to_string();
        for page in pages_of(&bytes) {
            let elements = analyze_page(&page.spans, page.size, &[], &page.rules, page.no);
            for element in &elements {
                let (bbox, what) = match element {
                    Element::Text {
                        bbox, label, text, ..
                    } => (
                        *bbox,
                        format!(
                            "text/{label} {:?}",
                            text.chars().take(40).collect::<String>()
                        ),
                    ),
                    Element::Table { bbox, rows, .. } => {
                        (*bbox, format!("table {}x{}", rows.len(), 1))
                    }
                    Element::Picture { bbox, .. } => (*bbox, "picture".to_string()),
                };
                let [l, t, r, b] = bbox;
                assert!(
                    bbox.iter().all(|v| v.is_finite()),
                    "{name} page {} {what}: non-finite box {bbox:?}",
                    page.no
                );
                assert!(
                    l >= 0.0 && t >= 0.0 && r <= page.size[0] && b <= page.size[1],
                    "{name} page {} {what}: box {bbox:?} leaves the {}x{} page",
                    page.no,
                    page.size[0],
                    page.size[1]
                );
                // Zero area is the signature of a box built from no geometry at
                // all. A hair-thin box is not: a `/` or `|` glyph really is that
                // narrow, and `irs_f1040.pdf` contains one.
                assert!(
                    r > l && b > t,
                    "{name} page {} {what}: degenerate box {bbox:?}",
                    page.no
                );
            }
        }
    }
}

/// The document-wide pass, over the corpus.
///
/// Furniture is decided across pages, so this is the only suite that can hold it
/// to the real paper: the pass relabels what the *whole document* repeats, and
/// nothing that a page can settle on its own.
#[test]
fn the_page_numbers_of_the_corpus_are_furniture_and_no_heading_stands_in_a_footer() {
    let mut total = 0usize;
    for path in corpus() {
        let bytes = std::fs::read(&path).expect("readable");
        let name = path
            .file_name()
            .expect("a name")
            .to_string_lossy()
            .to_string();
        let pages = pages_of(&bytes);
        let mut document: Vec<PageElements> = pages
            .iter()
            .map(|page| PageElements {
                size: page.size,
                elements: analyze_page(&page.spans, page.size, &[], &page.rules, page.no),
                rules: page.rules.clone(),
            })
            .collect();
        total += mark_furniture(&mut document);

        // The bare numbers standing in the pages' bottom margins, and which page
        // each of them stands on.
        let mut numbers: Vec<(i32, &str, &str)> = Vec::new();
        let mut numbered_pages: BTreeSet<i32> = BTreeSet::new();
        for (index, page) in document.iter().enumerate() {
            let band = page.size[1] * FURNITURE_BAND;
            for element in &page.elements {
                let Element::Text {
                    text, label, bbox, ..
                } = element
                else {
                    continue;
                };
                if bbox[1] < page.size[1] - band {
                    continue;
                }
                // Nothing that stands in a page's foot may be structure: a page
                // number read as a section heading is an outline entry that does
                // not exist (`arxiv_attention.pdf`'s `13` was one).
                assert!(
                    !matches!(label.as_str(), "title" | "section_header" | "list_item"),
                    "{name} page {}: {text:?} is a {label} in the footer band",
                    pages[index].no
                );
                if !text.trim().is_empty() && text.trim().chars().all(|c| c.is_ascii_digit()) {
                    numbers.push((pages[index].no, label, text));
                    numbered_pages.insert(pages[index].no);
                }
            }
        }
        // A number the document prints at the foot of more than one page is a
        // page number, and the pass has to have said so on every one of them.
        if numbered_pages.len() >= 2 {
            for (page, label, text) in numbers {
                assert_eq!(
                    label, "page_footer",
                    "{name} page {page}: the page number {text:?} is a {label}"
                );
            }
        }
    }
    // And the pass is not allowed to stop firing: the corpus is papers, and
    // papers number their pages.
    assert!(
        total >= 60,
        "only {total} item(s) of furniture across the corpus"
    );
}

/// Every footnote the corpus holds, by document, page and opening words.
///
/// A snapshot, because a note is the one thing here that is found by *two* marks at
/// once and a change to either moves the boundary between a note and body text. The
/// list is what the pass finds today; a note gained or lost has to be a deliberate
/// edit to this list, not a silent one. See `test-data/golden/README.md` for the notes
/// Docling finds and this pass does not — after the page's rules were given to layout
/// and the fonts' own encodings read, there are two, and both are `tracemonkey.pdf`'s,
/// whose page draws no rule above them.
const CORPUS_FOOTNOTES: [(&str, i32, &str); 23] = [
    (
        "arxiv_attention.pdf",
        1,
        "\u{2217}Equal contribution. Listing order is",
    ),
    (
        "arxiv_attention.pdf",
        1,
        "\u{2020}Work performed while at Google Brain",
    ),
    (
        "arxiv_attention.pdf",
        1,
        "\u{2021}Work performed while at Google Research",
    ),
    (
        "arxiv_attention.pdf",
        4,
        "4To illustrate why the dot products",
    ),
    ("arxiv_attention.pdf", 8, "5We used values of 2.8"),
    // The three author lines `math_gan.pdf` prints under no rule at all: their
    // markers are in the page's symbol font, whose codes as Latin-1 are nothing
    // (`∗`), `y` and `z`.
    ("math_gan.pdf", 1, "\u{2217}Jean Pouget-Abadie is visiting"),
    ("math_gan.pdf", 1, "\u{2020}Sherjil Ozair is visiting"),
    (
        "math_gan.pdf",
        1,
        "\u{2021}Yoshua Bengio is a CIFAR Senior Fellow",
    ),
    ("math_gan.pdf", 1, "1All code and hyperparameters"),
    (
        "math_layernorm.pdf",
        6,
        "1https://github.com/ivendrov/order-embedding",
    ),
    (
        "math_layernorm.pdf",
        7,
        "2https://github.com/cooijmanstim/Attentive_reader",
    ),
    (
        "math_layernorm.pdf",
        7,
        "3We only produce results on the validation",
    ),
    (
        "math_layernorm.pdf",
        8,
        "4https://github.com/ryankiros/skip-thoughts",
    ),
    ("math_layernorm.pdf", 8, "5https://github.com/NVIDIA/cnmem"),
    ("math_seq2seq.pdf", 1, "\u{2217}CIFAR Senior Fellow"),
    ("math_seq2seq.pdf", 2, "1 We mean by the state-of-the-art"),
    (
        "math_seq2seq.pdf",
        2,
        "2 Although most of the previous works",
    ),
    (
        "math_seq2seq.pdf",
        4,
        "3 http://www.statmt.org/wmt14/translation-task",
    ),
    ("math_seq2seq.pdf", 4, "4 Implementations are available at"),
    (
        "math_seq2seq.pdf",
        4,
        "5 Available online at http://www-lium",
    ),
    (
        "math_seq2seq.pdf",
        5,
        "6 We used the tokenization script from",
    ),
    (
        "math_seq2seq.pdf",
        5,
        "7 In this paper, by a \u{2019}hidden unit\u{2019}",
    ),
    ("math_seq2seq.pdf", 12, "8 Here, we show the formula"),
];

/// The notes the pass finds, held to the list above — and to the rule it says it
/// follows: each one stands under a mark the page drew and opens with a marker its
/// body also uses.
#[test]
fn the_corpus_footnotes_are_the_ones_the_rule_and_marker_find() {
    let mut found: Vec<(String, i32, String)> = Vec::new();
    for path in corpus() {
        let bytes = std::fs::read(&path).expect("readable");
        let name = path
            .file_name()
            .expect("a name")
            .to_string_lossy()
            .to_string();
        let pages = pages_of(&bytes);
        let mut document: Vec<PageElements> = pages
            .iter()
            .map(|page| PageElements {
                size: page.size,
                elements: analyze_page(&page.spans, page.size, &[], &page.rules, page.no),
                rules: page.rules.clone(),
            })
            .collect();
        // Furniture first, the order the CLI runs them in: a page number is the
        // page's, and the note pass must not be able to take one from it.
        mark_furniture(&mut document);
        mark_footnotes(&mut document);
        for (index, page) in document.iter().enumerate() {
            for element in &page.elements {
                let Element::Text {
                    text, label, bbox, ..
                } = element
                else {
                    continue;
                };
                if label != "footnote" {
                    continue;
                }
                // The pass says a note stands under a rule. If one of these does
                // not, the rule it was found by was somewhere else.
                let height = page.size[1];
                assert!(
                    page.rules.iter().any(|rule| {
                        rule[3] <= bbox[1] + 30.0
                            && rule[1] >= height / 2.0
                            && rule[0] < bbox[2]
                            && rule[2] > bbox[0]
                    }),
                    "{name} page {}: {text:?} is a footnote with no rule above it",
                    pages[index].no
                );
                found.push((name.clone(), pages[index].no, text.clone()));
            }
        }
    }

    assert_eq!(
        found.len(),
        CORPUS_FOOTNOTES.len(),
        "{} note(s), expected {}\n{found:#?}",
        found.len(),
        CORPUS_FOOTNOTES.len()
    );
    for ((name, page, text), (want_name, want_page, opening)) in found.iter().zip(CORPUS_FOOTNOTES)
    {
        assert_eq!(name, want_name, "the notes left this document");
        assert_eq!(*page, want_page, "{name}: the notes moved page");
        assert!(
            text.starts_with(opening),
            "{name} page {page}: expected a note opening {opening:?}, found {text:?}"
        );
    }
}

#[test]
fn blocks_are_ordered_top_to_bottom_within_a_single_column_page() {
    // `basicapi.pdf` is a three-page single-column API listing.
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data/real/basicapi.pdf");
    let bytes = std::fs::read(&path).expect("readable");
    for page in pages_of(&bytes) {
        let elements = analyze_page(&page.spans, page.size, &[], &page.rules, page.no);
        let tops: Vec<f64> = elements
            .iter()
            .filter_map(|e| match e {
                Element::Text { bbox, .. } => Some(bbox[1]),
                _ => None,
            })
            .collect();
        let mut sorted = tops.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
        assert_eq!(tops, sorted, "page {} is out of reading order", page.no);
    }
}
