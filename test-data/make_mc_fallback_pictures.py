#!/usr/bin/env python3
"""Write `mc_fallback_pictures.docx` and `mc_fallback_pictures.pptx`.

A picture may be stated only for an older reader: `mc:AlternateContent` with no
`mc:Choice`, only an `mc:Fallback`. That is the fallback-only form — the branch
LibreOffice imports when there is nothing in a choice to take, and the branch a
reader that looked only at `mc:Choice` would lose the picture to.

Each package holds a sentence, one 60 × 36 orange raster in the fallback, and a
sentence after it. Neither office library writes this form, so each package is
written with the raster in the picture's place and then completed by wrapping the
drawing or picture in the alternate content: a VML `v:imagedata` in the DOCX
fallback, a second `p:pic` in the PPTX one.

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
    document.save(path)


def write_pptx(path: Path) -> None:
    deck = Presentation()
    slide = deck.slides.add_slide(deck.slide_layouts[6])  # blank

    def words(top: float, sentence: str) -> None:
        box = slide.shapes.add_textbox(Inches(0.8), Inches(top), Inches(4.0), Inches(0.4))
        box.text_frame.text = sentence

    words(0.4, "Before the picture.")
    slide.shapes.add_picture(
        str(ORANGE), Inches(0.8), Inches(1.0), width=PptxPt(SIZE[0]), height=PptxPt(SIZE[1])
    )
    words(2.5, "After the picture.")
    deck.save(path)


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

    drawing = re.search(r"<w:drawing>.*?</w:drawing>", document, re.DOTALL)
    assert drawing, f"{path}: no drawing to put in a fallback"
    embed = re.search(r'r:embed="([^"]+)"', drawing.group(0))
    assert embed, f"{path}: a drawing with no blip"

    fallback = (
        "<mc:AlternateContent><mc:Fallback><w:pict>"
        '<v:shape id="FallbackPicture" filled="f" stroked="f" '
        f'style="width:{SIZE[0]:g}pt;height:{SIZE[1]:g}pt">'
        f'<v:imagedata r:id="{embed.group(1)}" o:title=""/>'
        "</v:shape></w:pict></mc:Fallback></mc:AlternateContent>"
    )
    document = document[: drawing.start()] + fallback + document[drawing.end() :]

    parts["word/document.xml"] = document.encode("utf-8")
    rewrite(path, parts)


def complete_pptx(path: Path) -> None:
    with zipfile.ZipFile(path) as package:
        parts = {name: package.read(name) for name in package.namelist()}

    slide = parts["ppt/slides/slide1.xml"].decode("utf-8")
    slide = declare(slide, "<p:sld", {"mc": MC})

    picture = re.search(r"<p:pic>.*?</p:pic>", slide, re.DOTALL)
    assert picture, f"{path}: no picture to put in a fallback"

    fallback = (
        "<mc:AlternateContent><mc:Fallback>"
        + picture.group(0)
        + "</mc:Fallback></mc:AlternateContent>"
    )
    slide = slide[: picture.start()] + fallback + slide[picture.end() :]

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
