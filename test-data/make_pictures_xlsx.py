#!/usr/bin/env python3
"""Write `test-data/worksheet_pictures.xlsx`, a workbook that draws pictures.

The workbook is written by `xlsxwriter` — a real XLSX writer, not a hand-built
package — so the shapes the parser meets are the ones a spreadsheet application
puts in the file: an `xl/drawings/drawing1.xml` per sheet, an `xdr:twoCellAnchor`
per picture with its `from`/`to` cell markers, a relationship part beside the
drawing naming the media parts, and the media parts themselves under
`xl/media/`:

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_pictures_xlsx.py

What it holds, and what each picture is there to prove:

* `Pictures` — the sheet a reader sees:
  * `chart.bmp` at A6 — a BMP is not an encoding Docling's office readers ever
    keep, so it has to reach the document as a PNG. `xlsxwriter` copies the BMP
    through byte for byte, so the fixture's own media part is the picture in
    `test-data/images/chart.bmp`, and the pixels written back can be held to it.
  * `chart.gif` at C6 — a GIF likewise, and a palette one: a wrong decode (the
    palette, or a frame smaller than the logical screen) is visible in the
    pixels, not just in the header.
  * `invoice_scan.png` at F6 — a PNG is handed on untouched, so the bytes beside
    the JSON have to be the part's own bytes.
  * `scan.jpg` at F20 — and so is a JPEG, which is the second encoding Docling's
    own readers leave a picture in. `xlsxwriter` keeps the file's `.jpeg`
    extension in the part name, so the alias is read here too.
* `Summary` — a second sheet with a picture of its own, so two worksheets mean
  two drawings: the picture of each is read (a reader that only looked at the
  workbook's first drawing would lose the rest of the file).

Both sheets are written by one writer and the values are fixed, so the file is
reproducible apart from the ZIP timestamps `xlsxwriter` writes; the tests pin
the parse and the picture files, never the container's bytes.
"""

import zipfile
from pathlib import Path

import xlsxwriter

HERE = Path(__file__).resolve().parent
IMAGES = HERE / "images"
OUT = HERE / "worksheet_pictures.xlsx"


def main() -> None:
    workbook = xlsxwriter.Workbook(OUT)
    workbook.set_properties({"title": "A workbook with pictures"})

    pictures = workbook.add_worksheet("Pictures")
    pictures.write("A1", "A workbook with pictures.")
    pictures.write("A3", "The table the pictures sit under.")
    pictures.write_row("A4", ["Item", "Qty"])
    pictures.write_row("A5", ["Widget", 12])
    pictures.write_row("A6", ["Gadget", 7])

    # The four pictures sit below the table, down the page and across it, so a
    # reading that ordered a sheet's items by position puts them where a reader
    # sees them: the text, then the pictures in the order they are drawn.
    pictures.insert_image("A9", str(IMAGES / "chart.bmp"))
    pictures.insert_image("C9", str(IMAGES / "chart.gif"))
    pictures.insert_image("F9", str(HERE / "invoice_scan.png"))
    pictures.insert_image("F23", str(IMAGES / "scan.jpg"))

    summary = workbook.add_worksheet("Summary")
    summary.write("A1", "A second sheet, with a picture of its own.")
    summary.insert_image("A3", str(IMAGES / "chart.bmp"))
    summary.set_column("A:A", 24)

    workbook.close()

    # The claim above — that the BMP is carried through as it is — is checked
    # here rather than assumed: a writer that re-encoded it would make the
    # fixture say something other than what its generator says it says.
    source = (IMAGES / "chart.bmp").read_bytes()
    carried = False
    with zipfile.ZipFile(OUT) as package:
        for name in package.namelist():
            if package.read(name) == source:
                carried = True
                break
    if not carried:
        raise SystemExit("the BMP was re-encoded by the writer; the fixture is not as described")


if __name__ == "__main__":
    main()
