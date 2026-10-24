#!/usr/bin/env python3
"""Write `blip_fill_shapes.docx` and `blip_fill_shapes.pptx`.

A shape can be filled with a picture instead of a colour, and Word and
PowerPoint state that fill the way they state a picture: the raster on the
`a:blip`, and, when the fill is an SVG, the drawing in an `asvg:svgBlip`
extension beside it:

    <a:blipFill>
      <a:blip r:embed="rId5"><a:extLst><a:ext uri="{96DAC541-…}">
        <asvg:svgBlip r:embed="rId6"/>
      </a:ext></a:extLst></a:blip>
      <a:stretch><a:fillRect/></a:stretch>
    </a:blipFill>

A reader that looks for a picture's `p:blipFill` and nothing else drops such a
shape whole: the fill is the only ink it has, and the raster twin is a picture
of the wrong thing. Word states the shape as a `wps:wsp` inside `w:drawing`;
PowerPoint states it as a `p:sp` whose `p:spPr` carries the `a:blipFill`.

Neither office library fills a shape with a picture, so each package is written
with the shape and a colour fill, and then completed the way an application
completes it: the fill's `a:blipFill` put in place of the colour, a second media
part for the drawing, a relationship naming it, a content-type declaration for
the extension, and the `asvg:svgBlip` extension inside the blip.

Each package holds two filled shapes and a sentence before and after them:

* the first is filled with `images/diagram.svg`, the 40 × 24 drawing the other
  SVG fixtures use, and keeps a grey 40 × 24 box as its raster fallback — the
  box is deliberately unlike the drawing, so a reader that takes the fallback is
  visible in what comes out. It also carries the words "Inside the fill.", so
  the shape's text is still read beside its picture;
* the second is filled with an orange 60 × 36 raster and states no drawing, so
  it proves the fill a reader has to take when there is no SVG at all.

Regenerate with:

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_blip_fill_shapes.py

The libraries are used only to write the fixtures; the readers under test never
see this script, and they read the committed bytes.
"""

import re
import zipfile
from pathlib import Path

from docx import Document
from docx.shared import Inches, Pt
from PIL import Image, ImageDraw
from pptx import Presentation
from pptx.enum.shapes import MSO_SHAPE
from pptx.util import Inches, Pt as PptxPt

HERE = Path(__file__).resolve().parent
IMAGES = HERE / "images"
SVG = IMAGES / "diagram.svg"
DOCX = HERE / "blip_fill_shapes.docx"
PPTX = HERE / "blip_fill_shapes.pptx"

SVG_EXTENSION_URI = "{96DAC541-7B7A-43D3-8B79-37D633B846F1}"
ASVG = "http://schemas.microsoft.com/office/drawing/2016/SVG/main"
WPS = "http://schemas.microsoft.com/office/word/2010/wordprocessingShape"

# What the two shapes are, in points: the drawing's own grid is 40 × 24, and the
# raster-filled shape is larger so the two pictures cannot be confused.
FIRST = (40.0, 24.0)
SECOND = (60.0, 36.0)

BOX = HERE / ".blip-fill-box.png"
ORANGE = HERE / ".blip-fill-orange.png"


def write_fallbacks() -> None:
    """The two rasters the packages carry as their fills."""
    box = Image.new("RGB", (int(FIRST[0]), int(FIRST[1])), (128, 128, 128))
    ImageDraw.Draw(box).rectangle(
        [(2, 2), (int(FIRST[0]) - 3, int(FIRST[1]) - 3)], outline=(64, 64, 64)
    )
    box.save(BOX)

    orange = Image.new("RGB", (int(SECOND[0]), int(SECOND[1])), (224, 140, 40))
    ImageDraw.Draw(orange).rectangle(
        [(4, 4), (int(SECOND[0]) - 5, int(SECOND[1]) - 5)], outline=(120, 70, 20)
    )
    orange.save(ORANGE)


def write_docx(path: Path) -> None:
    document = Document()
    document.add_paragraph("Before the filled shapes.")
    # One picture per shape, in the shape's place, to be turned into a shape's
    # fill below: the drawing holds the extent the shape is placed by.
    document.add_picture(str(BOX), width=Inches(2.0), height=Inches(1.2))
    document.add_picture(str(ORANGE), width=Pt(SECOND[0]), height=Pt(SECOND[1]))
    document.add_paragraph("After the filled shapes.")
    document.save(path)


def write_pptx(path: Path) -> None:
    deck = Presentation()
    slide = deck.slides.add_slide(deck.slide_layouts[6])  # blank

    before = slide.shapes.add_textbox(
        Inches(0.8), Inches(0.4), Inches(4.0), Inches(0.4)
    )
    before.text_frame.text = "Before the filled shapes."

    first = slide.shapes.add_shape(
        MSO_SHAPE.ROUNDED_RECTANGLE,
        Inches(0.8),
        Inches(1.0),
        Inches(2.0),
        Inches(1.2),
    )
    first.text_frame.text = "Inside the fill."

    slide.shapes.add_shape(
        MSO_SHAPE.ROUNDED_RECTANGLE,
        Inches(0.8),
        Inches(1.8),
        PptxPt(SECOND[0]),
        PptxPt(SECOND[1]),
    )

    after = slide.shapes.add_textbox(
        Inches(0.8), Inches(3.0), Inches(4.0), Inches(0.4)
    )
    after.text_frame.text = "After the filled shapes."

    deck.save(path)


def relationship_target(from_part: str, to_part: str) -> str:
    """`to_part`, as a relationship inside `from_part` names it."""
    assert to_part.startswith(("word/", "ppt/")), to_part
    if from_part.startswith("word/"):
        return to_part[len("word/") :]
    # A slide part's relationships resolve against `ppt/slides/`.
    return "../" + to_part[len("ppt/") :]


def add_part(
    parts: dict[str, bytes],
    drawing_part: str,
    rels_part: str,
    media_part: str,
    data: bytes,
    extension: str,
    mime: str,
) -> str:
    """Add a media part and the relationship naming it; return the relationship
    id, whether or not the part is written."""
    rels = parts[rels_part].decode("utf-8")
    used = [int(match.group(1)) for match in re.finditer(r'Id="rId(\d+)"', rels)]
    rel = f"rId{max(used) + 1}"
    parts[rels_part] = rels.replace(
        "</Relationships>",
        f'<Relationship Id="{rel}" '
        'Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" '
        f'Target="{relationship_target(drawing_part, media_part)}"/></Relationships>',
    ).encode("utf-8")
    if data:
        parts[media_part] = data

    types = parts["[Content_Types].xml"].decode("utf-8")
    if f'Extension="{extension}"' not in types:
        marker = '<Default Extension="rels"'
        assert marker in types, f"no rels default to insert beside the {extension} one"
        types = types.replace(
            marker, f'<Default Extension="{extension}" ContentType="{mime}"/>' + marker
        )
    parts["[Content_Types].xml"] = types.encode("utf-8")
    return rel


def blip_fill(fallback_rel: str, drawing_rel: str | None) -> str:
    """The `a:blipFill` a shape is filled with, drawing named when there is one."""
    extension = (
        f'<a:extLst><a:ext uri="{SVG_EXTENSION_URI}">'
        f'<asvg:svgBlip xmlns:asvg="{ASVG}" r:embed="{drawing_rel}"/>'
        "</a:ext></a:extLst>"
        if drawing_rel
        else ""
    )
    return (
        f'<a:blipFill><a:blip r:embed="{fallback_rel}">{extension}</a:blip>'
        "<a:stretch><a:fillRect/></a:stretch></a:blipFill>"
    )


def complete_docx(path: Path) -> None:
    with zipfile.ZipFile(path) as package:
        parts = {name: package.read(name) for name in package.namelist()}

    svg_rel = add_part(
        parts,
        "word/document.xml",
        "word/_rels/document.xml.rels",
        "word/media/diagram.svg",
        SVG.read_bytes(),
        "svg",
        "image/svg+xml",
    )

    document = parts["word/document.xml"].decode("utf-8")
    drawings = re.findall(r"<w:drawing>.*?</w:drawing>", document, re.DOTALL)
    assert len(drawings) == 2, f"{path}: expected two drawings, found {len(drawings)}"

    for index, (drawing, (width, height)) in enumerate(zip(drawings, [FIRST, SECOND])):
        embed = re.search(r'r:embed="([^"]+)"', drawing)
        assert embed, f"{path}: a drawing with no blip"
        extent = re.search(r'<wp:extent cx="(\d+)" cy="(\d+)"', drawing)
        assert extent, f"{path}: a drawing with no extent"
        cx, cy = extent.group(1), extent.group(2)
        drawing_rel = svg_rel if index == 0 else None
        words = (
            "<wps:txbx><w:txbxContent><w:p><w:r><w:t>Inside the fill.</w:t></w:r></w:p>"
            "</w:txbxContent></wps:txbx>"
            if index == 0
            else ""
        )
        document = document.replace(
            drawing,
            '<w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0">'
            f'<wp:extent cx="{cx}" cy="{cy}"/>'
            f'<wp:docPr id="{10 + index}" name="Filled shape {index + 1}"/>'
            "<wp:cNvGraphicFramePr/>"
            '<a:graphic><a:graphicData uri="' + WPS + '">'
            f'<wps:wsp xmlns:wps="{WPS}"><wps:cNvSpPr/><wps:spPr>'
            f'<a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm>'
            f'<a:prstGeom prst="roundRect"><a:avLst/></a:prstGeom>'
            + blip_fill(embed.group(1), drawing_rel)
            + '<a:ln w="12700"><a:solidFill><a:srgbClr val="2F6DB5"/>'
            "</a:solidFill></a:ln>"
            "</wps:spPr>" + words + "<wps:bodyPr/></wps:wsp>"
            "</a:graphicData></a:graphic></wp:inline></w:drawing>",
        )

    parts["word/document.xml"] = document.encode("utf-8")
    rewrite(path, parts)


def complete_pptx(path: Path) -> None:
    with zipfile.ZipFile(path) as package:
        parts = {name: package.read(name) for name in package.namelist()}

    slide_part = "ppt/slides/slide1.xml"
    rels_part = "ppt/slides/_rels/slide1.xml.rels"
    svg_rel = add_part(
        parts,
        slide_part,
        rels_part,
        "ppt/media/diagram.svg",
        SVG.read_bytes(),
        "svg",
        "image/svg+xml",
    )
    # The rasters each shape is filled with, beside the drawing the first one
    # states: python-pptx fills a shape from its style and carries no picture.
    rasters = [
        add_part(
            parts,
            slide_part,
            rels_part,
            f"ppt/media/fill{index + 1}.png",
            raster.read_bytes(),
            "png",
            "image/png",
        )
        for index, raster in enumerate([BOX, ORANGE])
    ]

    slide = parts[slide_part].decode("utf-8")
    shapes = re.findall(r"<p:sp>.*?</p:sp>", slide, re.DOTALL)
    # The two autoshapes carry a `p:style` whose `fillRef` a colour would come
    # from; the text boxes state no fill and no style.
    filled = [shape for shape in shapes if "<p:style>" in shape]
    assert len(filled) == 2, f"{path}: expected two filled shapes, found {len(filled)}"

    for index, shape in enumerate(filled):
        drawing_rel = svg_rel if index == 0 else None
        # The fill belongs after the geometry and before the outline, which is
        # the order DrawingML's own schema states.
        filled_shape = shape.replace(
            "</a:prstGeom>",
            "</a:prstGeom>" + blip_fill(rasters[index], drawing_rel),
            1,
        )
        slide = slide.replace(shape, filled_shape)

    parts[slide_part] = slide.encode("utf-8")
    rewrite(path, parts)


def rewrite(path: Path, parts: dict[str, bytes]) -> None:
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as package:
        for name, data in parts.items():
            package.writestr(name, data)


def main() -> None:
    assert SVG.exists(), f"write {SVG} first (make_gif_svg_pictures.py)"
    write_fallbacks()
    write_docx(DOCX)
    write_pptx(PPTX)
    complete_docx(DOCX)
    complete_pptx(PPTX)
    BOX.unlink()
    ORANGE.unlink()
    for path in (DOCX, PPTX):
        print(f"wrote {path} ({path.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
