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
  * `scan.jpc` at F37 — a bare JPEG 2000 codestream, the picture a scanner or an
    encoder writes without the `jp2` container around it. No pixel format the
    `image` crate decodes, so it is the one picture here whose bytes have to be
    *decoded* to be read: a reader that only named the part would place a box
    with no ink in it. `xlsxwriter` cannot place a format it has no size for, so
    the fifth anchor, its relationship and its media part are written into the
    saved package the way a spreadsheet application writes them.
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

# The bare codestream, and where it goes in the saved package: the fifth picture
# of the `Pictures` sheet's drawing, below the scan it is the lossless twin of.
JPC = IMAGES / "scan.jpc"
JPC_PART = "xl/media/image5.jpc"
JPC_REL = "rId5"
DRAWING = "xl/drawings/drawing1.xml"
DRAWING_RELS = "xl/drawings/_rels/drawing1.xml.rels"
IMAGE_REL_TYPE = (
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image"
)


def add_jpc_picture() -> None:
    """Place `scan.jpc` on the `Pictures` sheet, as a spreadsheet app would."""
    with zipfile.ZipFile(OUT) as package:
        parts = {name: package.read(name) for name in package.namelist()}

    if JPC_PART in parts:
        raise SystemExit(f"{JPC_PART} is already in the workbook")
    parts[JPC_PART] = JPC.read_bytes()

    rels = parts[DRAWING_RELS].decode("utf-8")
    if JPC_REL in rels:
        raise SystemExit(f"{DRAWING_RELS} already names {JPC_REL}")
    parts[DRAWING_RELS] = rels.replace(
        "</Relationships>",
        f'<Relationship Id="{JPC_REL}" Type="{IMAGE_REL_TYPE}" '
        f'Target="../media/image5.jpc"/>'
        "</Relationships>",
    ).encode("utf-8")

    # The same 620 x 350 box the scan's own anchor states, at F37: from column 5
    # to column 11 and 0.1995 of the twelfth, from row 36 to row 47 and 0.2 of
    # the next — the box the JPEG above it occupies, fourteen rows further down.
    anchor = (
        '<xdr:twoCellAnchor editAs="oneCell">'
        "<xdr:from><xdr:col>5</xdr:col><xdr:colOff>0</xdr:colOff>"
        "<xdr:row>36</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from>"
        "<xdr:to><xdr:col>11</xdr:col><xdr:colOff>121920</xdr:colOff>"
        "<xdr:row>47</xdr:row><xdr:rowOff>38100</xdr:rowOff></xdr:to>"
        "<xdr:pic><xdr:nvPicPr>"
        '<xdr:cNvPr id="6" name="Picture 5" descr="scan.jpc"/>'
        '<xdr:cNvPicPr><a:picLocks noChangeAspect="1"/></xdr:cNvPicPr>'
        "</xdr:nvPicPr><xdr:blipFill>"
        '<a:blip xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/'
        f'relationships" r:embed="{JPC_REL}"/>'
        "<a:stretch><a:fillRect/></a:stretch></xdr:blipFill><xdr:spPr>"
        '<a:xfrm><a:off x="3048000" y="6858000"/>'
        '<a:ext cx="3779520" cy="2133600"/></a:xfrm>'
        '<a:prstGeom prst="rect"><a:avLst/></a:prstGeom>'
        "</xdr:spPr></xdr:pic><xdr:clientData/></xdr:twoCellAnchor>"
    )
    drawing = parts[DRAWING].decode("utf-8")
    if "scan.jpc" in drawing:
        raise SystemExit(f"{DRAWING} already draws scan.jpc")
    parts[DRAWING] = drawing.replace(
        "</xdr:wsDr>", anchor + "</xdr:wsDr>"
    ).encode("utf-8")

    # A media part's extension is the only statement the package makes about what
    # it holds, so the codestream's is declared like the other four.
    types = parts["[Content_Types].xml"].decode("utf-8")
    if 'Extension="jpc"' not in types:
        parts["[Content_Types].xml"] = types.replace(
            "</Types>",
            '<Default Extension="jpc" ContentType="image/jp2"/></Types>',
        ).encode("utf-8")

    with zipfile.ZipFile(OUT, "w", zipfile.ZIP_DEFLATED) as package:
        for name, data in parts.items():
            package.writestr(name, data)


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

    add_jpc_picture()

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
