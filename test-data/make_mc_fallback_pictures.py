#!/usr/bin/env python3
"""Write `mc_fallback_pictures.docx` and `mc_fallback_pictures.pptx`.

A picture may be stated only for an older reader: `mc:AlternateContent` with no
`mc:Choice`, only an `mc:Fallback`. That is the fallback-only form — the branch
LibreOffice imports when there is nothing in a choice to take, and the branch a
reader that looked only at `mc:Choice` would lose the picture to.

Each package holds a sentence, one 60 × 36 orange raster in the fallback, and a
sentence after it — then the same raster again inside a container a reader has
to walk into rather than meet in the body: a table cell in the DOCX, where the
`w:drawing` sits in the cell's own paragraph, and a `p:grpSp` group on the
slide, where the picture is placed through the group's transform. Neither
office library writes the fallback-only form, so each package is written with
the raster in the picture's place and then completed by wrapping the drawing or
picture in the alternate content: a VML `v:imagedata` in the DOCX fallback, a
second `p:pic` in the PPTX one.

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
from docx.shared import Pt
from PIL import Image, ImageDraw
from pptx import Presentation
from pptx.oxml import parse_xml
from pptx.oxml.ns import nsdecls, qn
from pptx.util import Inches, Pt as PptxPt

HERE = Path(__file__).resolve().parent
DOCX = HERE / "mc_fallback_pictures.docx"
PPTX = HERE / "mc_fallback_pictures.pptx"

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
    document.save(path)


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

    document = parts["word/document.xml"].decode("utf-8")
    document = declare(document, "<w:document", {"mc": MC, "v": V, "o": O})

    def fallback_of(drawing: re.Match) -> str:
        embed = re.search(r'r:embed="([^"]+)"', drawing.group(0))
        assert embed, f"{path}: a drawing with no blip"
        # VML names every shape, and two shapes may not share a name.
        name = f"FallbackPicture{embed.group(1)}"
        return (
            "<mc:AlternateContent><mc:Fallback><w:pict>"
            f'<v:shape id="{name}" filled="f" stroked="f" '
            f'style="width:{SIZE[0]:g}pt;height:{SIZE[1]:g}pt">'
            f'<v:imagedata r:id="{embed.group(1)}" o:title=""/>'
            "</v:shape></w:pict></mc:Fallback></mc:AlternateContent>"
        )

    document, drawings = re.subn(
        r"<w:drawing>.*?</w:drawing>", fallback_of, document, flags=re.DOTALL
    )
    assert drawings == 2, f"{path}: {drawings} drawing(s) to put in a fallback"

    parts["word/document.xml"] = document.encode("utf-8")
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
