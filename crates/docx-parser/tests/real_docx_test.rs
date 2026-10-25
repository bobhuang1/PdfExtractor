//! Regression tests against real `.docx` files produced by Word, LibreOffice
//! and other tools. See `test-data/real/README.md` for provenance.

use pdfextractor_docx_parser::{DocxDocument, DocxItem};
use std::path::PathBuf;

fn real(name: &str) -> Vec<u8> {
    let path = PathBuf::from("../../test-data/real").join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("missing corpus file {}: {e}", path.display()))
}

fn parse(name: &str) -> DocxDocument {
    DocxDocument::parse_bytes(&real(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn formulas(doc: &DocxDocument) -> Vec<String> {
    doc.items
        .iter()
        .filter_map(|i| match i {
            DocxItem::Formula { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn body_text(doc: &DocxDocument) -> String {
    doc.items
        .iter()
        .filter_map(|i| match i {
            DocxItem::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every `math-*.docx` fixture carries Office Math that must survive as text.
/// Recognising nothing here is the failure mode this guards.
#[test]
fn every_math_fixture_yields_a_formula() {
    let names = [
        "math-mso2k7.docx",
        "math-subscripts.docx",
        "math-matrix.docx",
        "math-rad.docx",
        "math-nary.docx",
        "math-accents.docx",
        "math-lim.docx",
        "math-placeholders.docx",
        "math-bold-italic.docx",
        "math-overstrike.docx",
        "math-vertical_stacks.docx",
        "math-escaping.docx",
        "math-literal.docx",
        "EquationWithAboveAndBelowCaption.docx",
        "EquationAsScientificNumbering.docx",
    ];
    for name in names {
        let doc = parse(name);
        let found = formulas(&doc);
        assert!(!found.is_empty(), "{name}: no formula extracted");
        assert!(
            found.iter().all(|f| !f.trim().is_empty()),
            "{name}: empty formula"
        );
    }
}

/// Radicals keep their degree, and the degree is an optional argument.
#[test]
fn renders_radicals_as_sqrt() {
    let doc = parse("math-rad.docx");
    assert_eq!(
        formulas(&doc),
        vec!["\\sqrt{4}", "\\sqrt[3]{x+1}"],
        "radicals do not match Docling's LaTeX"
    );
}

/// The n-ary operator character lives in an XML *attribute*
/// (`<m:chr m:val="∑"/>`); reading only child text silently loses it. The
/// command plus `_{…}^{…}` limits are exactly Docling's rendering.
#[test]
fn nary_operators_come_from_attributes() {
    let doc = parse("math-nary.docx");
    assert_eq!(
        formulas(&doc),
        vec!["\\oiiint_{1}^{2}x+1", "\\prod_{a}^{}b", "\\sum_{}^{2}x"],
        "n-ary operators do not match Docling's LaTeX"
    );
}

/// A real published equation, pinned to Docling's own conversion of it.
#[test]
fn recovers_the_binomial_theorem() {
    let doc = parse("EquationWithAboveAndBelowCaption.docx");
    let found = formulas(&doc);
    assert_eq!(
        found[0],
        "\\left(x+a\\right)^{n}=\\sum_{k=0}^{n}\\left(\\genfrac{}{}{0pt}{}{n}{k}\\right)x^{k}a^{n-k}",
        "the theorem does not match Docling's LaTeX"
    );
    assert!(
        found[1].starts_with("\\left(1+x\\right)^{n}=1+\\frac{nx}{1!}"),
        "the expansion lost its fractions: {found:?}"
    );
    assert!(
        found[1].ends_with("\\text{ \\textellipsis }"),
        "the trailing ellipsis is not LaTeX: {found:?}"
    );
}

/// Scripts, matrices, the four fraction styles, functions and limits use the
/// commands Docling writes for the same constructs.
#[test]
fn writes_structures_with_doclings_commands() {
    let scripts = formulas(&parse("math-subscripts.docx"));
    assert_eq!(
        &scripts[..5],
        ["x^{y}+e^{x}", "x^{b}", "x_{b}", "a_{c}^{b}", "_{2}^{1}"]
    );

    let matrix = formulas(&parse("math-matrix.docx"));
    assert_eq!(
        matrix,
        vec!["\\left[\\begin{matrix}1&2\\\\3&4\\end{matrix}\\right]"]
    );

    let stacks = formulas(&parse("math-vertical_stacks.docx"));
    assert_eq!(
        stacks,
        vec![
            "\\frac{a}{b}",
            "{a}/{b}",
            "\\genfrac{}{}{0pt}{}{a}{b}",
            "\\genfrac{}{}{0pt}{}{a}{\\genfrac{}{}{0pt}{}{b}{c}}",
        ]
    );

    let limit = formulas(&parse("math-lim.docx"));
    assert_eq!(limit, vec!["\\lim_{x \\text{ \\textrightarrow } 1}x"]);
}

/// A symbol is written as its command, not as the Unicode character: Greek
/// letters, relations, accents and delimiters all survive as LaTeX.
#[test]
fn writes_symbols_and_accents_as_commands() {
    let accents = formulas(&parse("math-accents.docx"));
    assert_eq!(
        accents,
        vec!["\\acute{a}\\grave{a}\\check{a}\\breve{a}\\ocirc{a}\\vec{a}\\tilde{a}\\hat{a}˙\\vec{a}\\vec{a}\\tilde{a}\\underline{a}"]
    );

    let literal = formulas(&parse("math-literal.docx"));
    assert_eq!(
        literal,
        vec!["\\iiint_{V}^{}divFdV=\\oiint_{S}^{}F \\bullet n dS"]
    );

    let escaping = formulas(&parse("math-escaping.docx"));
    assert_eq!(escaping, vec!["- \\infty <x< \\infty"]);
}

/// Headings, list items and tables are labelled rather than flattened.
#[test]
fn labels_headings_lists_and_tables() {
    let doc = parse("calibre_demo.docx");
    let headers = doc
        .items
        .iter()
        .filter(|i| matches!(i, DocxItem::Text { label, .. } if label == "section_header"))
        .count();
    let lists = doc
        .items
        .iter()
        .filter(|i| matches!(i, DocxItem::Text { label, .. } if label == "list_item"))
        .count();
    let tables = doc
        .items
        .iter()
        .filter(|i| matches!(i, DocxItem::Table(_)))
        .count();
    assert!(headers > 0, "no headings found");
    assert!(lists > 0, "no list items found");
    assert!(tables > 0, "no tables found");

    // A table must be a real grid, not a single flattened cell.
    let table = doc
        .items
        .iter()
        .find_map(|i| match i {
            DocxItem::Table(table) => Some(&table.rows),
            _ => None,
        })
        .expect("a table");
    assert!(table.len() > 1, "table has one row: {table:?}");
    assert!(table.iter().any(|r| r.len() > 1), "table has one column");
}

/// Images come out of `word/media/`, resolved through the drawing's
/// relationship id, with the placement size the document asked for.
#[test]
fn extracts_embedded_images() {
    let doc = parse("VariousPictures.docx");
    let pictures: Vec<_> = doc
        .items
        .iter()
        .filter_map(|i| match i {
            DocxItem::Picture {
                mime,
                data,
                width_pt,
                ..
            } => Some((*mime, data.len(), *width_pt)),
            _ => None,
        })
        .collect();
    assert_eq!(
        pictures.len(),
        5,
        "expected the five drawings to be reported"
    );
    assert!(
        pictures.iter().all(|(_, len, _)| *len > 0),
        "an image came out empty: {pictures:?}"
    );
    assert!(
        pictures
            .iter()
            .any(|(mime, _, _)| mime.starts_with("image/")),
        "no recognised image type: {pictures:?}"
    );
    // At least one drawing states its size in EMU; we must convert to points.
    assert!(
        pictures
            .iter()
            .any(|(_, _, pt)| pt.map(|v| v > 1.0).unwrap_or(false)),
        "no placement size was read: {pictures:?}"
    );
}

/// A drawing Word stored as a metafile is a list of GDI calls, not a pixel
/// grid. `image/emf` and `image/wmf` cannot be named in an image reference, so
/// the three drawings in this document used to be reported as boxes with no
/// image; they are played into PNGs instead, at the size the document asks for.
#[test]
fn plays_metafile_drawings_into_pngs() {
    let doc = parse("VariousPictures.docx");
    let pictures: Vec<(&str, &[u8])> = doc
        .items
        .iter()
        .filter_map(|i| match i {
            DocxItem::Picture { mime, data, .. } => Some((*mime, data.as_slice())),
            _ => None,
        })
        .collect();
    assert_eq!(pictures.len(), 5, "expected the five drawings");
    assert!(
        !pictures
            .iter()
            .any(|(mime, _)| *mime == "image/emf" || *mime == "image/wmf"),
        "a drawing is still carried as a metafile: {:?}",
        pictures.iter().map(|(mime, _)| *mime).collect::<Vec<_>>()
    );
    // The rasterised drawings join the picture that was a PNG to begin with.
    let pngs: Vec<&[u8]> = pictures
        .iter()
        .filter(|(mime, _)| *mime == "image/png")
        .map(|(_, data)| *data)
        .collect();
    assert_eq!(pngs.len(), 4, "three played drawings and the original PNG");
    for data in pngs {
        assert!(
            data.starts_with(&[0x89, b'P', b'N', b'G']),
            "a picture is named a PNG without being one"
        );
    }
}

/// An embedded object's preview is a metafile too, reached through `w:object`
/// and `v:imagedata` rather than a `w:drawing`. In `mathtype.docx` it is the
/// only picture the equation has: dropping it loses the equation entirely.
#[test]
fn plays_an_embedded_objects_preview() {
    let doc = parse("mathtype.docx");
    let (mime, data, width_pt, height_pt) = doc
        .items
        .iter()
        .find_map(|i| match i {
            DocxItem::Picture {
                mime,
                data,
                width_pt,
                height_pt,
            } => Some((*mime, data.as_slice(), *width_pt, *height_pt)),
            _ => None,
        })
        .expect("the object's preview is a picture");
    assert_eq!(mime, "image/png");
    assert!(data.starts_with(&[0x89, b'P', b'N', b'G']), "not a PNG");
    // `w:object/@w:dxaOrig` and `@w:dyaOrig` state the object's original size in
    // twips; 617 x 566 twips is the 30.85 x 28.3 pt the VML shape asks for.
    assert!(
        (width_pt.expect("a declared width") - 30.85).abs() < 0.01,
        "width {width_pt:?}"
    );
    assert!(
        (height_pt.expect("a declared height") - 28.3).abs() < 0.01,
        "height {height_pt:?}"
    );
    let width = u32::from_be_bytes(data[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(data[20..24].try_into().unwrap());
    assert_eq!(
        (width, height),
        (62, 57),
        "the raster is not the declared size at 144 dpi"
    );
    assert_eq!(body_text(&doc), "Beforeafter.", "the prose around it");
}

/// Pictures inside a header live in a separate part; dropping them would lose
/// the only image in the document.
#[test]
fn keeps_header_content_as_furniture() {
    let doc = parse("headerPic.docx");
    assert!(
        doc.furniture
            .iter()
            .any(|i| matches!(i, DocxItem::Picture { .. })),
        "header picture was dropped: {:?}",
        doc.furniture.len()
    );
    assert!(
        doc.items.is_empty()
            || doc
                .items
                .iter()
                .all(|i| !matches!(i, DocxItem::Picture { .. })),
        "header picture leaked into the body"
    );
}

/// The page size comes from `w:sectPr`, so the reported page is not a guess.
#[test]
fn reports_a4_page_size() {
    let doc = parse("drawing.docx");
    // These fixtures are A4; a Letter fallback would be 612x792.
    assert!(
        (doc.page_width - 595.3).abs() < 2.0 && (doc.page_height - 841.9).abs() < 2.0,
        "unexpected page size {}x{}",
        doc.page_width,
        doc.page_height
    );
}

fn texts(doc: &DocxDocument) -> Vec<(String, String)> {
    doc.items
        .iter()
        .filter_map(|i| match i {
            DocxItem::Text { text, label, .. } => Some((label.clone(), text.clone())),
            _ => None,
        })
        .collect()
}

/// A chart's title is written only in the chart part; the drawing in the
/// document body is a reference to it, so a reader looking at the drawing finds
/// no words at all. Six charts, six titles (bug 46).
#[test]
fn reads_chart_titles_from_the_chart_parts() {
    let doc = parse("chartex.docx");
    let captions: Vec<String> = texts(&doc)
        .into_iter()
        .filter(|(label, _)| label == "caption")
        .map(|(_, text)| text)
        .collect();
    for title in [
        "my chart looks nice",
        "This is a line chart I just made",
        "This is a radar chart",
        "This is a stock chart",
    ] {
        assert!(
            captions.iter().any(|c| c == title),
            "missing {title:?}: {captions:?}"
        );
    }
    // The two modern chart types write their title in the `cx:` namespace,
    // which Docling does not read. Reporting them is content, not noise.
    assert!(captions
        .iter()
        .any(|c| c == "this is a box and whisker chart"));
    assert!(captions.iter().any(|c| c == "this is a sunburst chart"));
    assert!(
        captions.len() >= 6,
        "only {} chart titles: {captions:?}",
        captions.len()
    );
}

/// Word writes every shape twice and a table of contents separates its title
/// from its page number with a tab; both were read wrong (bug 46).
#[test]
fn shapes_are_not_doubled_and_tabs_survive() {
    let doc = parse("shapes-with-text.docx");
    let blocks: Vec<String> = texts(&doc).into_iter().map(|(_, text)| text).collect();
    assert_eq!(
        blocks,
        vec![
            "Floating text box",
            "A square shape with text inside",
            "An ellipse with text inside",
            "A group of shapes",
            "Where some contain text",
        ],
        "each shape should appear once"
    );

    let contents = parse("calibre_demo.docx");
    let toc: Vec<String> = texts(&contents)
        .into_iter()
        .map(|(_, text)| text)
        .filter(|text| text.starts_with("Demonstration of DOCX"))
        .collect();
    assert!(
        toc.iter().any(|line| line.contains('\t')),
        "the table of contents lost its tab: {toc:?}"
    );
}

/// Footnote and endnote text is body content; Docling reports both as
/// `footnote` items at the end of the document (bug 46).
#[test]
fn footnotes_and_endnotes_come_out_as_prose() {
    let doc = parse("calibre_demo.docx");
    let notes: Vec<String> = texts(&doc)
        .into_iter()
        .filter(|(label, _)| label == "footnote")
        .map(|(_, text)| text)
        .collect();
    assert!(
        notes.iter().any(|t| t.contains("paged media")),
        "the footnote was dropped: {notes:?}"
    );
    assert!(
        notes
            .iter()
            .any(|t| t.contains("Endnotes are typically used")),
        "the endnote was dropped: {notes:?}"
    );
}

/// The document body must not lose its prose while tables are extracted.
#[test]
fn keeps_prose_alongside_tables() {
    let doc = parse("drawing.docx");
    let text = body_text(&doc);
    let tables = count_tables(&doc.items);
    assert!(text.len() > 500, "expected prose, got {} chars", text.len());
    assert!(tables > 5, "expected many tables, got {tables}");
}

/// Every table the document states, counting the ones written inside a cell:
/// a nested table is its cell's content rather than an item of the document,
/// so counting the items alone would undercount what the file holds.
fn count_tables(items: &[DocxItem]) -> usize {
    fn in_cells(table: &pdfextractor_docx_parser::DocxTable) -> usize {
        table
            .rows
            .iter()
            .flatten()
            .flat_map(|cell| &cell.tables)
            .map(|nested| 1 + in_cells(nested))
            .sum()
    }
    items
        .iter()
        .filter_map(|item| match item {
            DocxItem::Table(table) => Some(1 + in_cells(table)),
            _ => None,
        })
        .sum()
}
