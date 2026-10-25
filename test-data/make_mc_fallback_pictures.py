#!/usr/bin/env python3
"""Write `mc_fallback_pictures.docx` and `mc_fallback_pictures.pptx`.

A picture may be stated only for an older reader: `mc:AlternateContent` with no
`mc:Choice`, only an `mc:Fallback`. That is the fallback-only form — the branch
LibreOffice imports when there is nothing in a choice to take, and the branch a
reader that looked only at `mc:Choice` would lose the picture to.

Each package holds a sentence, one 60 × 36 orange raster in the fallback, and a
sentence after it — then the same raster again inside a container a reader has
to walk into rather than meet in the body. In the DOCX there are four such
places: a table cell, a table nested in another table's cell, a VML text box,
and a header part, which is read as furniture rather than as body content. On
the slide the picture is inside a `p:grpSp` group, where it is placed through
the group's transform. Neither office library writes the fallback-only form, so
each package is written with the raster in the picture's place and then
completed by wrapping the drawing or picture in the alternate content: a VML
`v:imagedata` in the DOCX fallback, a second `p:pic` in the PPTX one.

LibreOffice reads the fallback and keeps it: its DOCX export writes an
`mc:Choice` beside the fallback and leaves the fallback in place, while its PPTX
export writes the fallback as the slide's picture. Either way the 60 × 36 orange
picture is in what it wrote, which is what
`libreoffice_opens_and_resaves_the_fallback_only_fixture_packages` holds it to.

Regenerate with:

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_mc_fallback_pictures.py

The libraries are used only to write the fixtures; the readers under test never
see this script, and they read the committed bytes.
"""

import re
import zipfile
from pathlib import Path

from docx import Document
from docx.oxml import parse_xml
from docx.shared import Pt
from PIL import Image, ImageDraw
from pptx import Presentation
from pptx.oxml import parse_xml
from pptx.oxml.ns import nsdecls, qn
from pptx.util import Inches, Pt as PptxPt

HERE = Path(__file__).resolve().parent
DOCX = HERE / "mc_fallback_pictures.docx"
PPTX = HERE / "mc_fallback_pictures.pptx"

W = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
MC = "http://schemas.openxmlformats.org/markup-compatibility/2006"
V = "urn:schemas-microsoft-com:vml"
O = "urn:schemas-microsoft-com:office:office"

# The picture's own grid, in points.
SIZE = (60.0, 36.0)

ORANGE = HERE / ".mc-fallback-orange.png"


def write_picture() -> None:
    """The raster the fallback carries: deliberately not a drawing."""
    orange = Image.new("RGB", (int(SIZE[0]), int(SIZE[1])), (224, 140, 40))
    ImageDraw.Draw(orange).rectangle(
        [(4, 4), (int(SIZE[0]) - 5, int(SIZE[1]) - 5)], outline=(120, 70, 20)
    )
    orange.save(ORANGE)


def write_docx(path: Path) -> None:
    document = Document()
    document.add_paragraph("Before the picture.")
    document.add_picture(str(ORANGE), width=Pt(SIZE[0]), height=Pt(SIZE[1]))
    document.add_paragraph("After the picture.")

    # The same picture in a table cell: alternate content is read wherever the
    # picture stands, and a cell is a container the reader has to walk into
    # rather than a paragraph it meets in the body.
    document.add_paragraph("Before the table picture.")
    table = document.add_table(rows=1, cols=1)
    table.style = "Table Grid"
    table.cell(0, 0).paragraphs[0].add_run().add_picture(
        str(ORANGE), width=Pt(SIZE[0]), height=Pt(SIZE[1])
    )
    document.add_paragraph("After the table picture.")

    # The same picture in a table whose cell holds another table: the reader has
    # to walk into two tables to reach it, and the picture is the inner cell's.
    document.add_paragraph("Before the nested table picture.")
    outer = document.add_table(rows=1, cols=1)
    outer.style = "Table Grid"
    outer.cell(0, 0).paragraphs[0].text = "The outer cell."
    inner = outer.cell(0, 0).add_table(rows=1, cols=1)
    inner.cell(0, 0).paragraphs[0].add_run().add_picture(
        str(ORANGE), width=Pt(SIZE[0]), height=Pt(SIZE[1])
    )
    document.add_paragraph("After the nested table picture.")

    # The same picture in a text box: the shape is a `w:pict`, and the picture
    # is a drawing inside the shape's own `w:txbxContent`, one container further
    # in than a cell's paragraph.
    document.add_paragraph("Before the text box picture.")
    add_text_box(document)
    document.add_paragraph("After the text box picture.")

    # And the same picture in a header: its own part, read as furniture rather
    # than as body content.
    header = document.sections[0].header
    header.paragraphs[0].text = "The header."
    header.paragraphs[0].add_run().add_picture(
        str(ORANGE), width=Pt(SIZE[0]), height=Pt(SIZE[1])
    )
    document.save(path)


def add_text_box(document) -> None:
    """Put the picture in a VML text box, its content a paragraph of the drawing."""
    holder = document.add_paragraph()
    run = holder.add_run()
    run.add_picture(str(ORANGE), width=Pt(SIZE[0]), height=Pt(SIZE[1]))
    drawing = run._element[-1]
    run._element.remove(drawing)
    box = parse_xml(
        f'<w:pict xmlns:w="{W}" xmlns:v="{V}" xmlns:o="{O}">'
        '<v:shape id="FallbackTextBox" type="#_x0000_t202" '
        'style="width:120pt;height:60pt" filled="f" stroked="t">'
        '<v:textbox><w:txbxContent><w:p><w:r/></w:p></w:txbxContent></v:textbox>'
        '</v:shape></w:pict>'
    )
    inner_run = box.find(".//{%s}r" % W)
    assert inner_run is not None, "the text box has no run to hold the picture"
    inner_run.append(drawing)
    run._element.append(box)


def write_pptx(path: Path) -> None:
    deck = Presentation()
    slide = deck.slides.add_slide(deck.slide_layouts[6])  # blank

    def words(top: float, sentence: str) -> None:
        box = slide.shapes.add_textbox(Inches(0.8), Inches(top), Inches(4.0), Inches(0.4))
        box.text_frame.text = sentence

    words(0.3, "Before the picture.")
    slide.shapes.add_picture(
        str(ORANGE), Inches(0.8), Inches(0.7), width=PptxPt(SIZE[0]), height=PptxPt(SIZE[1])
    )
    words(1.6, "After the picture.")

    # The same picture inside a group: the picture is placed through the
    # group's transform, so a reader that stops at the body loses it.
    words(2.4, "Before the group picture.")
    picture = slide.shapes.add_picture(
        str(ORANGE), Inches(0.8), Inches(2.8), width=PptxPt(SIZE[0]), height=PptxPt(SIZE[1])
    )
    group_around(picture)
    words(4.4, "After the group picture.")
    deck.save(path)


def group_around(picture) -> None:
    """Move `picture` into a `p:grpSp` of its own, its transform the picture's."""
    element = picture._element
    xfrm = element.find(".//" + qn("a:xfrm"))
    off = xfrm.find(qn("a:off"))
    ext = xfrm.find(qn("a:ext"))
    x, y = int(off.get("x")), int(off.get("y"))
    cx, cy = int(ext.get("cx")), int(ext.get("cy"))
    group = parse_xml(
        f'<p:grpSp {nsdecls("p", "a")}><p:nvGrpSpPr>'
        f'<p:cNvPr id="100" name="Picture group"/><p:cNvGrpSpPr/><p:nvPr/>'
        f'</p:nvGrpSpPr><p:grpSpPr><a:xfrm>'
        f'<a:off x="{x}" y="{y}"/><a:ext cx="{cx}" cy="{cy}"/>'
        f'<a:chOff x="{x}" y="{y}"/><a:chExt cx="{cx}" cy="{cy}"/>'
        f'</a:xfrm></p:grpSpPr></p:grpSp>'
    )
    element.getparent().replace(element, group)
    group.append(element)


def declare(xml: str, root: str, prefixes: dict[str, str]) -> str:
    """Add any of `prefixes` the root element `root` does not declare."""
    start = xml.index(root)
    end = xml.index(">", start)
    tag = xml[start:end]
    missing = "".join(
        f' xmlns:{prefix}="{uri}"'
        for prefix, uri in prefixes.items()
        if f"xmlns:{prefix}=" not in tag
    )
    return xml[:end] + missing + xml[end:]


def complete_docx(path: Path) -> None:
    with zipfile.ZipFile(path) as package:
        parts = {name: package.read(name) for name in package.namelist()}

    def fallback_of(drawing: re.Match) -> str:
        embed = re.search(r'r:embed="([^"]+)"', drawing.group(0))
        assert embed, f"{path}: a drawing with no blip"
        # VML names every shape, and two shapes may not share a name.
        fallback_of.serial += 1
        name = f"FallbackPicture{embed.group(1)}_{fallback_of.serial}"
        return (
            "<mc:AlternateContent><mc:Fallback><w:pict>"
            f'<v:shape id="{name}" filled="f" stroked="f" '
            f'style="width:{SIZE[0]:g}pt;height:{SIZE[1]:g}pt">'
            f'<v:imagedata r:id="{embed.group(1)}" o:title=""/>'
            "</v:shape></w:pict></mc:Fallback></mc:AlternateContent>"
        )

    fallback_of.serial = 0
    # The body's drawing part, and every header or footer: a header's picture is
    # stated in its own part, and a reader that only completed the body would
    # leave that one a plain drawing.
    drawing_parts = ["word/document.xml"] + sorted(
        name
        for name in parts
        if name.startswith(("word/header", "word/footer")) and name.endswith(".xml")
    )
    for name in drawing_parts:
        xml = parts[name].decode("utf-8")
        root = (
            "<w:document"
            if name == "word/document.xml"
            else "<w:hdr" if "/header" in name else "<w:ftr"
        )
        xml = declare(xml, root, {"mc": MC, "v": V, "o": O})
        xml, drawings = re.subn(
            r"<w:drawing>.*?</w:drawing>", fallback_of, xml, flags=re.DOTALL
        )
        assert drawings > 0, f"{path}: {name} holds no picture to put in a fallback"
        parts[name] = xml.encode("utf-8")

    assert fallback_of.serial == 5, (
        f"{path}: {fallback_of.serial} drawing(s), not the body's four and the header's one"
    )
    rewrite(path, parts)


def complete_pptx(path: Path) -> None:
    with zipfile.ZipFile(path) as package:
        parts = {name: package.read(name) for name in package.namelist()}

    slide = parts["ppt/slides/slide1.xml"].decode("utf-8")
    slide = declare(slide, "<p:sld", {"mc": MC})

    fallback = (
        "<mc:AlternateContent><mc:Fallback>"
        + r"\g<0>"
        + "</mc:Fallback></mc:AlternateContent>"
    )
    slide, pictures = re.subn(r"<p:pic>.*?</p:pic>", fallback, slide, flags=re.DOTALL)
    assert pictures == 2, f"{path}: {pictures} picture(s) to put in a fallback"

    parts["ppt/slides/slide1.xml"] = slide.encode("utf-8")
    rewrite(path, parts)


def rewrite(path: Path, parts: dict[str, bytes]) -> None:
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as package:
        for name, data in parts.items():
            package.writestr(name, data)


def main() -> None:
    write_picture()
    write_docx(DOCX)
    write_pptx(PPTX)
    complete_docx(DOCX)
    complete_pptx(PPTX)
    ORANGE.unlink()
    for path in (DOCX, PPTX):
        print(f"wrote {path} ({path.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
