#!/usr/bin/env python3
"""Write test-data/merged_cells.docx: the merged-cell table fixture.

A purpose-built DOCX (an OPC package of XML parts, zipped) whose one table
declares every merge WordprocessingML can:

  * `w:gridSpan`   — the header cell "Results" spans two grid columns;
  * `w:vMerge`     — each class letter ("A", "B") opens a vertical merge the
                     row below continues;
  * `w:tblHeader`  — the first row is marked as the header.

The goldens hold the readings apart: HTML writes the merges as native
`rowspan`/`colspan`, Markdown (and text) flatten them, and YAML lists the
spans in the document model. Regenerate the file with:

    python3 test-data/make_merged_cells_docx.py

and re-run the golden tests with UPDATE_GOLDEN_MD=1 / UPDATE_GOLDEN_FORMATS=1
if the table's shape ever changes.
"""

import zipfile

W = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"

CONTENT_TYPES = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>
"""

ROOT_RELS = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>
"""

DOCUMENT_RELS = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"/>
"""


def paragraph(text, style=None):
    props = f"<w:pPr><w:pStyle w:val=\"{style}\"/></w:pPr>" if style else ""
    return f"<w:p>{props}<w:r><w:t>{text}</w:t></w:r></w:p>"


def cell(text, props=""):
    body = f"<w:p><w:r><w:t>{text}</w:t></w:r></w:p>" if text else "<w:p/>"
    return f"<w:tc><w:tcPr>{props}</w:tcPr>{body}</w:tc>"


WIDTH = '<w:tcW w:w="2000" w:type="dxa"/>'
# Class | Student | Results spanning Test and Exam.
HEADER = (
    "<w:tr><w:trPr><w:tblHeader/></w:trPr>"
    + cell("Class", WIDTH)
    + cell("Student", WIDTH)
    + cell("Results", WIDTH + '<w:gridSpan w:val="2"/>')
    + "</w:tr>"
)


def merged_row(letter, restart, student, test, exam):
    """One body row: the class letter opens (or continues) its vertical merge."""
    class_cell = (
        cell(letter, WIDTH + '<w:vMerge w:val="restart"/>')
        if restart
        else cell("", WIDTH + "<w:vMerge/>")
    )
    return (
        "<w:tr>"
        + class_cell
        + cell(student, WIDTH)
        + cell(test, WIDTH)
        + cell(exam, WIDTH)
        + "</w:tr>"
    )


TABLE = (
    "<w:tbl>"
    + "<w:tblGrid>"
    + '<w:gridCol w:w="2000"/><w:gridCol w:w="2000"/>'
    + '<w:gridCol w:w="2000"/><w:gridCol w:w="2000"/>'
    + "</w:tblGrid>"
    + HEADER
    + merged_row("A", True, "Alice", "71", "84")
    + merged_row("", False, "Bob", "65", "79")
    + merged_row("B", True, "Carol", "90", "88")
    + merged_row("", False, "Dave", "77", "82")
    + "</w:tbl>"
)

DOCUMENT = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="{W}">
<w:body>
{paragraph("Scores by class", "Heading1")}
{paragraph("The header spans two columns, and each class letter runs down two rows.")}
{TABLE}
{paragraph("Rows without a class letter continue the one above.")}
</w:body>
</w:document>
"""


def main():
    path = "test-data/merged_cells.docx"
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as package:
        # Fixed timestamps, so regenerating the fixture byte-for-byte.
        def put(name, text):
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            package.writestr(info, text)

        put("[Content_Types].xml", CONTENT_TYPES)
        put("_rels/.rels", ROOT_RELS)
        put("word/_rels/document.xml.rels", DOCUMENT_RELS)
        put("word/document.xml", DOCUMENT)
    print(f"wrote {path}")


if __name__ == "__main__":
    main()
