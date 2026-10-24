//! The parser against the committed fixture, `test-data/inventory.xlsx`.
//!
//! The unit tests build packages by hand to pin one rule each; this one reads a
//! workbook a real writer (`openpyxl`, see `test-data/make_inventory_xlsx.py`)
//! produced, so the shapes a real package puts in the file — inline strings,
//! the date's serial and style, the data validation beside the cells — are read
//! as they are written rather than as the tests imagine them.

use pdfextractor_xlsx_parser::XlsxDocument;
use std::path::PathBuf;

fn fixture() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data/inventory.xlsx");
    std::fs::read(&path).unwrap_or_else(|e| panic!("missing {}: {e}", path.display()))
}

#[test]
fn the_workbook_reads_as_three_sheets_with_their_tables() {
    let document = XlsxDocument::parse_bytes(&fixture()).expect("parse");

    let names: Vec<(&str, bool)> = document
        .sheets
        .iter()
        .map(|sheet| (sheet.name.as_str(), sheet.hidden))
        .collect();
    assert_eq!(
        names,
        vec![("Inventory", false), ("Notes", false), ("Hidden", true)]
    );

    let counts: Vec<usize> = document
        .sheets
        .iter()
        .map(|sheet| sheet.tables.len())
        .collect();
    // The Inventory sheet's empty row leaves its cells three regions: the
    // header block, the merged note, and the "Total" cell whose formula has no
    // cached value to read.
    assert_eq!(counts, vec![3, 1, 1]);
}

#[test]
fn the_header_block_is_one_table_with_a_merged_note_below_it() {
    let document = XlsxDocument::parse_bytes(&fixture()).expect("parse");
    let inventory = &document.sheets[0];

    let header = &inventory.tables[0];
    assert_eq!(header.anchor, (0, 0));
    assert_eq!((header.num_rows, header.num_cols), (3, 4));
    let first_row: Vec<&str> = header
        .cells
        .iter()
        .filter(|cell| cell.row == 0)
        .map(|cell| cell.text.as_str())
        .collect();
    assert_eq!(first_row, vec!["Item", "Qty", "Price", "In stock"]);
    // A boolean is its Python spelling, because that is the text Docling writes
    // for the cell a spreadsheet reader shows as TRUE.
    assert!(header
        .cells
        .iter()
        .any(|cell| cell.row == 1 && cell.text == "True"));
    assert!(header
        .cells
        .iter()
        .any(|cell| cell.row == 2 && cell.text == "False"));

    let note = &inventory.tables[1];
    assert_eq!(note.anchor, (1, 4));
    assert_eq!((note.num_rows, note.num_cols), (1, 2));
    assert_eq!(note.cells.len(), 1);
    assert_eq!(note.cells[0].text, "Shipping included");
    assert_eq!(note.cells[0].col_span, 2);
}

/// The pictures a sheet draws come through the parts that hold them: the
/// worksheet's `<drawing>`, that drawing's own relationship part, and the media
/// part each anchor reaches. The cells around them are still read as table
/// regions, so the fixture is a workbook rather than a bag of pictures.
#[test]
fn each_sheet_draws_the_pictures_its_own_drawing_part_anchors() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data/worksheet_pictures.xlsx");
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("missing {}: {e}", path.display()));
    let document = XlsxDocument::parse_bytes(&data).expect("parse");

    let names: Vec<&str> = document
        .sheets
        .iter()
        .map(|sheet| sheet.name.as_str())
        .collect();
    assert_eq!(names, vec!["Pictures", "Summary"]);

    let kinds: Vec<Vec<(&str, [f64; 4])>> = document
        .sheets
        .iter()
        .map(|sheet| {
            sheet
                .pictures
                .iter()
                .map(|picture| (picture.mime, picture.bbox))
                .collect()
        })
        .collect();
    // `Pictures` draws five, in the order its drawing part anchors them; each
    // box is the rectangle the anchor names, in cells, offsets inside a cell
    // included: the BMP spans one column less than the GIF starts at, and the
    // scans run into the column after their last one by 0.1995 of a cell.
    assert_eq!(kinds[0].len(), 5, "{:#?}", kinds[0]);
    let expected = [
        (
            "image/bmp",
            [0.0, 8.0, 571_500.0 / 609_600.0, 9.0 + 152_400.0 / 190_500.0],
        ),
        (
            "image/gif",
            [
                2.0,
                8.0,
                2.0 + 571_500.0 / 609_600.0,
                9.0 + 152_400.0 / 190_500.0,
            ],
        ),
        (
            "image/png",
            [
                5.0,
                8.0,
                11.0 + 121_608.0 / 609_600.0,
                19.0 + 37_924.0 / 190_500.0,
            ],
        ),
        (
            "image/jpeg",
            [
                5.0,
                22.0,
                11.0 + 121_920.0 / 609_600.0,
                33.0 + 38_100.0 / 190_500.0,
            ],
        ),
        // A bare codestream: the part's extension is the only name the package
        // gives it, so the reader takes it as the JPEG 2000 it is.
        (
            "image/jp2",
            [
                5.0,
                36.0,
                11.0 + 121_920.0 / 609_600.0,
                47.0 + 38_100.0 / 190_500.0,
            ],
        ),
    ];
    for (actual, (mime, bbox)) in kinds[0].iter().zip(expected) {
        assert_eq!(actual.0, mime, "{:#?}", kinds[0]);
        for (edge, wanted) in actual.1.iter().zip(bbox) {
            assert!(
                (edge - wanted).abs() < 1e-9,
                "{mime} is at {:#?}, not {bbox:?}",
                actual.1
            );
        }
    }

    // The second sheet draws a picture of its own: a reader that only looked at
    // the first drawing would lose the rest of the workbook.
    assert_eq!(kinds[1].len(), 1, "{:#?}", kinds[1]);
    assert_eq!(kinds[1][0].0, "image/bmp");

    // And the media parts are the bytes the package carries: the writer copied
    // the same bitmap into both sheets, so both are that one file.
    let bmp = std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data/images/chart.bmp"),
    )
    .expect("the BMP fixture");
    assert_eq!(document.sheets[0].pictures[0].data, bmp);
    assert_eq!(document.sheets[1].pictures[0].data, bmp);

    // The cells are still a workbook's cells: two regions on the sheet, the
    // text and the small table above the pictures.
    let tables = &document.sheets[0].tables;
    assert_eq!(tables.len(), 2);
    assert_eq!(tables[0].cells[0].text, "A workbook with pictures.");
    assert_eq!(tables[1].anchor, (0, 2));
}

#[test]
fn a_date_is_the_text_a_reader_sees() {
    let document = XlsxDocument::parse_bytes(&fixture()).expect("parse");
    let notes = &document.sheets[1].tables[0];
    let updated = notes
        .cells
        .iter()
        .find(|cell| cell.row == 2 && cell.col == 1)
        .expect("the date cell");
    // The file stores 45356 with a date format; a reader sees the date.
    assert_eq!(updated.text, "2024-03-05 00:00:00");
}

/// A chart a sheet draws is not a media part but a chart part, and what it says
/// is the data it plots. The fixture is written by `openpyxl` (see
/// `test-data/make_chart_sheet_xlsx.py`), which leaves the numbers in the cells
/// and writes only references into the chart part — so this is also the gate on
/// reading a reference against the workbook rather than requiring a cache.
#[test]
fn each_sheet_reads_the_chart_its_drawing_frames() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data/chart_sheet.xlsx");
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("missing {}: {e}", path.display()));
    let document = XlsxDocument::parse_bytes(&data).expect("parse");

    let names: Vec<&str> = document
        .sheets
        .iter()
        .map(|sheet| sheet.name.as_str())
        .collect();
    assert_eq!(names, vec!["Quarterly", "Trend"]);

    // A chart is not a picture: the sheet draws none, and the media directory of
    // the package is empty.
    assert!(document
        .sheets
        .iter()
        .all(|sheet| sheet.pictures.is_empty()));

    let quarterly = &document.sheets[0].charts;
    assert_eq!(quarterly.len(), 1, "{quarterly:#?}");
    let bar = &quarterly[0];
    assert_eq!(bar.title.as_deref(), Some("Revenue by region"));
    assert_eq!(bar.kind.class_name(), "bar_chart");
    assert_eq!(bar.categories, vec!["North", "South", "West"]);
    let series: Vec<(&str, Vec<Option<f64>>)> = bar
        .series
        .iter()
        .map(|series| (series.name.as_str(), series.values.clone()))
        .collect();
    assert_eq!(
        series,
        vec![
            ("Q1", vec![Some(120.0), Some(90.0), Some(60.0)]),
            ("Q2", vec![Some(140.0), Some(95.0), Some(80.0)]),
        ]
    );
    // Anchored at `E2` with the size the writer gave it: column 4, row 1, and
    // the box it spans in cells.
    let bbox = bar.bbox;
    assert_eq!((bbox[0], bbox[1]), (4.0, 1.0));
    assert!(
        (bbox[2] - (4.0 + 5_400_000.0 / 609_600.0)).abs() < 1e-9,
        "{bbox:?}"
    );
    assert!(
        (bbox[3] - (1.0 + 2_700_000.0 / 190_500.0)).abs() < 1e-9,
        "{bbox:?}"
    );

    // The second sheet's chart is read too — a reader that only looked at the
    // first drawing would lose it — and it has no title, which is not a failure
    // to read one.
    let trend = &document.sheets[1].charts;
    assert_eq!(trend.len(), 1, "{trend:#?}");
    assert_eq!(trend[0].title, None);
    assert_eq!(trend[0].kind.class_name(), "line_chart");
    assert_eq!(trend[0].categories, vec!["North", "South", "West"]);
    assert_eq!(
        trend[0].series,
        vec![pdfextractor_chart::ChartSeries {
            name: "Q1".to_string(),
            values: vec![Some(120.0), Some(90.0), Some(60.0)],
        }]
    );

    // The cells are still a workbook's cells: one table per sheet, the numbers
    // the charts were drawn from.
    assert_eq!(document.sheets[0].tables.len(), 1);
    assert_eq!(document.sheets[0].tables[0].anchor, (0, 0));
    assert_eq!(document.sheets[0].tables[0].cells[0].text, "Region");
}
