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
  * `chart.bmp` at A9 — a BMP is not an encoding Docling's office readers ever
    keep, so it has to reach the document as a PNG. `xlsxwriter` copies the BMP
    through byte for byte, so the fixture's own media part is the picture in
    `test-data/images/chart.bmp`, and the pixels written back can be held to it.
  * `chart.gif` at C9 — a GIF likewise, and a palette one: a wrong decode (the
    palette, or a frame smaller than the logical screen) is visible in the
    pixels, not just in the header.
  * `invoice_scan.png` at F9 — a PNG is handed on untouched, so the bytes beside
    the JSON have to be the part's own bytes.
  * `scan.jpg` at F23 — and so is a JPEG, which is the second encoding Docling's
    own readers leave a picture in. `xlsxwriter` keeps the file's `.jpeg`
    extension in the part name, so the alias is read here too.
  * `scan.jpc` at F37 — a bare JPEG 2000 codestream, the picture a scanner or an
    encoder writes without the `jp2` container around it. No pixel format the
    `image` crate decodes, so it is one of the pictures here whose bytes have to
    be *decoded* to be read: a reader that only named the part would place a box
    with no ink in it.
  * `scan.jp2` at F51 — the same photograph in the container: a JP2 is a tree of
    boxes whose `jp2h` states the grid and whose `jp2c` carries the codestream,
    so it is the spelling whose size is written in the file rather than in the
    stream's own `SIZ` marker. Read as a picture it is the same pixels as the
    codestream beside it; read as a name alone it is a box with no ink.

  `xlsxwriter` cannot place a format it has no size for, so the fifth and sixth
  anchors, their relationships and their media parts are written into the saved
  package the way a spreadsheet application writes them.
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

DRAWING = "xl/drawings/drawing1.xml"
DRAWING_RELS = "xl/drawings/_rels/drawing1.xml.rels"
IMAGE_REL_TYPE = (
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image"
)

# The two spellings of the same photograph, and where each goes in the saved
# package: the container beside the bare codestream, fourteen rows below the
# scan whose pixels both hold. `row` is the anchor's first row, counted from
# zero the way the drawing part counts it.
JPEG2000 = [
    # source, part, relationship, row, shape id, name, description
    (IMAGES / "scan.jpc", "xl/media/image5.jpc", "rId5", 36, 6, "Picture 5", "scan.jpc"),
    (IMAGES / "scan.jp2", "xl/media/image6.jp2", "rId6", 50, 7, "Picture 6", "scan.jp2"),
]


def add_jpeg2000_pictures() -> None:
    """Place both JPEG 2000 spellings on the `Pictures` sheet, as an app would."""
    with zipfile.ZipFile(OUT) as package:
        parts = {name: package.read(name) for name in package.namelist()}

    drawing = parts[DRAWING].decode("utf-8")
    rels = parts[DRAWING_RELS].decode("utf-8")
    types = parts["[Content_Types].xml"].decode("utf-8")

    for source, part, rel, row, shape_id, name, description in JPEG2000:
        if part in parts:
            raise SystemExit(f"{part} is already in the workbook")
        parts[part] = source.read_bytes()

        if rel in rels:
            raise SystemExit(f"{DRAWING_RELS} already names {rel}")
        rels = rels.replace(
            "</Relationships>",
            f'<Relationship Id="{rel}" Type="{IMAGE_REL_TYPE}" '
            f'Target="../media/{part.rsplit("/", 1)[1]}"/>'
            "</Relationships>",
        )

        # The same 620 x 350 box the scan's own anchor states: from column 5 to
        # column 11 and 0.1995 of the twelfth, from this row to the eleventh
        # after it and 0.2 of the next.
        anchor = (
            '<xdr:twoCellAnchor editAs="oneCell">'
            "<xdr:from><xdr:col>5</xdr:col><xdr:colOff>0</xdr:colOff>"
            f"<xdr:row>{row}</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from>"
            "<xdr:to><xdr:col>11</xdr:col><xdr:colOff>121920</xdr:colOff>"
            f"<xdr:row>{row + 11}</xdr:row><xdr:rowOff>38100</xdr:rowOff></xdr:to>"
            "<xdr:pic><xdr:nvPicPr>"
            f'<xdr:cNvPr id="{shape_id}" name="{name}" descr="{description}"/>'
            '<xdr:cNvPicPr><a:picLocks noChangeAspect="1"/></xdr:cNvPicPr>'
            "</xdr:nvPicPr><xdr:blipFill>"
            '<a:blip xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/'
            f'relationships" r:embed="{rel}"/>'
            "<a:stretch><a:fillRect/></a:stretch></xdr:blipFill><xdr:spPr>"
            f'<a:xfrm><a:off x="3048000" y="{row * 190500}"/>'
            '<a:ext cx="3779520" cy="2133600"/></a:xfrm>'
            '<a:prstGeom prst="rect"><a:avLst/></a:prstGeom>'
            "</xdr:spPr></xdr:pic><xdr:clientData/></xdr:twoCellAnchor>"
        )
        if description in drawing:
            raise SystemExit(f"{DRAWING} already draws {description}")
        drawing = drawing.replace("</xdr:wsDr>", anchor + "</xdr:wsDr>")

        # A media part's extension is the only statement the package makes about
        # what it holds, so each spelling's is declared like the other four.
        extension = description.rsplit(".", 1)[1]
        if f'Extension="{extension}"' not in types:
            types = types.replace(
                "</Types>",
                f'<Default Extension="{extension}" ContentType="image/jp2"/></Types>',
            )

    parts[DRAWING] = drawing.encode("utf-8")
    parts[DRAWING_RELS] = rels.encode("utf-8")
    parts["[Content_Types].xml"] = types.encode("utf-8")

    with zipfile.ZipFile(OUT, "w", zipfile.ZIP_DEFLATED) as package:
        for part, data in parts.items():
            package.writestr(part, data)


def main() -> None:
    workbook = xlsxwriter.Workbook(OUT)
    workbook.set_properties({"title": "A workbook with pictures"})

    pictures = workbook.add_worksheet("Pictures")
    pictures.write("A1", "A workbook with pictures.")
    pictures.write("A3", "The table the pictures sit under.")
    pictures.write_row("A4", ["Item", "Qty"])
    pictures.write_row("A5", ["Widget", 12])
    pictures.write_row("A6", ["Gadget", 7])

    # The pictures sit below the table, down the page and across it, so a
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

    add_jpeg2000_pictures()

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
