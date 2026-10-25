#!/usr/bin/env python3
"""Write `picture_bullets.docx` and `picture_bullets.pptx`: list items whose
bullets are pictures.

A list level usually prints a glyph, but it can print a picture instead. Word
names one with `w:lvlPicBulletId` -> `w:numPicBullet`, PowerPoint with an
`a:buBlip` in the paragraph's properties. Either way the bullet is a media part
rather than a character, so a reader that looks only for glyphs loses it: the
item is left unmarked, or the drawing is dropped whole.

Both fixtures carry both spellings of the picture:

* a drawing, `images/diagram.svg` (the same 40 x 24 one the other office
  fixtures use), named the way Word and PowerPoint 2016 and later state an SVG —
  a grey PNG fallback on the `a:blip`, the drawing itself in an `asvg:svgBlip`
  extension beside it. The fallback is deliberately a featureless grey box, so a
  reader that stops at `r:embed` is visible in what comes out;
* a plain raster through the picture's own `r:embed`/`r:id`, which is a picture
  already and must be reported as stored.

In Word the two bullets are two list levels, each with its own frame: the
drawing's in `wp:extent` (18 points square), the VML raster's in the `v:shape`
`style` (10 x 7 points). In PowerPoint they are two paragraphs of one text box,
each `a:buBlip` a bullet; the drawing's bullet is drawn at that paragraph's
24-point type size and the raster's at the 18-point default, because a marker is
drawn at the size of the line it opens.

Neither office library can place a picture bullet, so both packages are written
with the pictures in place and then completed by hand: the media parts, the
relationships naming them, the content-type declaration for each extension that
is added (the deck the library writes declares no picture type it did not use),
and the bullet element in the markup. Regenerate with:

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_picture_bullets.py

Pillow is used only to write the raster parts of the fixtures; the readers under
test never see this script, and they read the committed bytes.
"""

import io
import re
import zipfile
from pathlib import Path

from PIL import Image, ImageDraw
from pptx import Presentation
from pptx.util import Inches as PptxInches
from pptx.util import Pt

HERE = Path(__file__).resolve().parent
DOCX = HERE / "picture_bullets.docx"
PPTX = HERE / "picture_bullets.pptx"
SVG = HERE / "images" / "diagram.svg"

W = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
R = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
WP = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
A = "http://schemas.openxmlformats.org/drawingml/2006/main"
PIC = "http://schemas.openxmlformats.org/drawingml/2006/picture"
ASVG = "http://schemas.microsoft.com/office/drawing/2016/SVG/main"
VML = "urn:schemas-microsoft-com:vml"
SVG_EXTENSION_URI = "{96DAC541-7B7A-43D3-8B79-37D633B846F1}"
IMAGE_REL = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image"

# The drawing's bullet frame in Word: 18 points square, in English Metric Units.
BULLET_EMU = 228600
# The VML raster bullet's frame in Word: 10 x 7 points.
RASTER_SIZE = (10, 7)
# The raster bullet's own pixels.
RASTER_PIXELS = (24, 16)
RASTER_COLOUR = (192, 32, 32)
# The PowerPoint items' type sizes: the drawing's bullet is 24-point text, the
# raster's paragraph states none and gets the 18-point body default.
DRAWN_PT = 24
DEFAULT_PT = 18


def fallback_png() -> bytes:
    """A grey box: what the drawing's raster fallback looks like when nobody
    drew the drawing. Nothing about it is the SVG's pale panel or blue line."""
    box = Image.new("RGB", (40, 24), (128, 128, 128))
    draw = ImageDraw.Draw(box)
    draw.rectangle([(1, 1), (38, 22)], outline=(96, 96, 96))
    out = io.BytesIO()
    box.save(out, format="PNG")
    return out.getvalue()


def raster_png() -> bytes:
    """The raster bullet: a solid block, so a re-encode or a misplaced pixel is
    visible in the picture that comes out."""
    image = Image.new("RGB", RASTER_PIXELS, RASTER_COLOUR)
    out = io.BytesIO()
    image.save(out, format="PNG")
    return out.getvalue()


# --- Word ------------------------------------------------------------------

CONTENT_TYPES = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Default Extension="png" ContentType="image/png"/>
  <Default Extension="svg" ContentType="image/svg+xml"/>
  <Override PartName="/word/document.xml"
            ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
  <Override PartName="/word/numbering.xml"
            ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/>
</Types>
"""

PACKAGE_RELS = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="{R}/officeDocument" Target="word/document.xml"/>
</Relationships>
"""

DOCUMENT_RELS = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="{R}/numbering" Target="numbering.xml"/>
</Relationships>
"""

# A bullet's picture is a part of `numbering.xml`, so Word names it from that
# part's own relationships — not the document's, where `rId1` is the numbering
# part itself. A reader that looks only at the document's rels loses the bullet.
NUMBERING_RELS = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="{R}/image" Target="media/bullet-fallback.png"/>
  <Relationship Id="rId2" Type="{R}/image" Target="media/bullet.svg"/>
  <Relationship Id="rId3" Type="{R}/image" Target="media/star.png"/>
</Relationships>
"""

DOCUMENT = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="{W}" xmlns:r="{R}"><w:body>
  <w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr>
    <w:r><w:t>Drawn bullet item</w:t></w:r></w:p>
  <w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="2"/></w:numPr></w:pPr>
    <w:r><w:t>Painted bullet item</w:t></w:r></w:p>
  <w:sectPr><w:pgSz w:w="12240" w:h="15840"/></w:sectPr>
</w:body></w:document>
"""

NUMBERING = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:numbering xmlns:w="{W}" xmlns:r="{R}" xmlns:wp="{WP}" xmlns:a="{A}" xmlns:pic="{PIC}"
             xmlns:asvg="{ASVG}" xmlns:v="{VML}">
  <w:numPicBullet w:numPicBulletId="0">
    <w:drawing><wp:inline><wp:extent cx="{BULLET_EMU}" cy="{BULLET_EMU}"/>
      <wp:docPr id="1" name="Picture bullet"/>
      <a:graphic><a:graphicData uri="{PIC}"><pic:pic>
        <pic:nvPicPr><pic:cNvPr id="1" name="bullet"/><pic:cNvPicPr/></pic:nvPicPr>
        <pic:blipFill>
          <a:blip r:embed="rId1"><a:extLst>
            <a:ext uri="{SVG_EXTENSION_URI}"><asvg:svgBlip r:embed="rId2"/></a:ext>
          </a:extLst></a:blip>
          <a:stretch><a:fillRect/></a:stretch>
        </pic:blipFill>
        <pic:spPr><a:xfrm><a:off x="0" y="0"/>
          <a:ext cx="{BULLET_EMU}" cy="{BULLET_EMU}"/></a:xfrm>
          <a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr>
      </pic:pic></a:graphicData></a:graphic>
    </wp:inline></w:drawing>
  </w:numPicBullet>
  <w:numPicBullet w:numPicBulletId="1">
    <w:pict><v:shape id="raster-bullet"
                     style="width:{RASTER_SIZE[0]}pt;height:{RASTER_SIZE[1]}pt">
      <v:imagedata r:id="rId3"/></v:shape></w:pict>
  </w:numPicBullet>
  <w:abstractNum w:abstractNumId="0">
    <w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="bullet"/>
      <w:lvlPicBulletId w:val="0"/></w:lvl>
  </w:abstractNum>
  <w:abstractNum w:abstractNumId="1">
    <w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="bullet"/>
      <w:lvlPicBulletId w:val="1"/></w:lvl>
  </w:abstractNum>
  <w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
  <w:num w:numId="2"><w:abstractNumId w:val="1"/></w:num>
</w:numbering>
"""


def write_zip(path: Path, parts: list[tuple[str, bytes | str]]) -> None:
    """Write a package whose timestamps are fixed: the fixture is committed, and
    a re-run that only changed the clock is not a change."""
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as package:
        for name, payload in parts:
            if isinstance(payload, str):
                payload = payload.encode("utf-8")
            info = zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            package.writestr(info, payload)


def write_docx() -> None:
    write_zip(
        DOCX,
        [
            ("[Content_Types].xml", CONTENT_TYPES),
            ("_rels/.rels", PACKAGE_RELS.format(R=R)),
            ("word/document.xml", DOCUMENT),
            ("word/_rels/document.xml.rels", DOCUMENT_RELS.format(R=R)),
            ("word/_rels/numbering.xml.rels", NUMBERING_RELS.format(R=R)),
            ("word/numbering.xml", NUMBERING),
            ("word/media/bullet-fallback.png", fallback_png()),
            ("word/media/bullet.svg", SVG.read_bytes()),
            ("word/media/star.png", raster_png()),
        ],
    )


# --- PowerPoint ------------------------------------------------------------

SLIDE = "ppt/slides/slide1.xml"
SLIDE_RELS = "ppt/slides/_rels/slide1.xml.rels"


def next_relationship_id(rels: str) -> str:
    used = [int(match.group(1)) for match in re.finditer(r'Id="rId(\d+)"', rels)]
    return f"rId{max(used, default=0) + 1}"


def declare_media_types(parts: dict[str, bytes]) -> None:
    """Declare each extension the media parts added here are named by.

    A part whose extension no `[Content_Types].xml` entry names has no media
    type, and is a file in the ZIP rather than a picture the presentation holds,
    however lenient a reader is about it. The deck the library writes declares
    only the extensions it drew with, so both of these are added here.
    """
    types = parts["[Content_Types].xml"].decode("utf-8")
    for extension, mime in (("png", "image/png"), ("svg", "image/svg+xml")):
        if f'Extension="{extension}"' not in types:
            types = types.replace(
                "</Types>",
                f'<Default Extension="{extension}" ContentType="{mime}"/></Types>',
            )
    parts["[Content_Types].xml"] = types.encode("utf-8")


def with_picture_bullets(slide: str, bullets: list[str]) -> str:
    """Put each paragraph's `a:buBlip` at the head of that paragraph."""
    opening = "<a:p>"
    pieces = slide.split(opening)
    rebuilt = [pieces[0]]
    for index, piece in enumerate(pieces[1:]):
        bullet = bullets[index] if index < len(bullets) else ""
        head = f"<a:pPr>{bullet}</a:pPr>" if bullet else ""
        rebuilt.append(opening + head + piece)
    return "".join(rebuilt)


def write_pptx() -> None:
    presentation = Presentation()
    slide = presentation.slides.add_slide(presentation.slide_layouts[6])  # blank
    box = slide.shapes.add_textbox(
        PptxInches(0.8), PptxInches(0.6), PptxInches(4.0), PptxInches(2.6)
    )
    frame = box.text_frame
    frame.text = "Drawn bullet item"
    frame.paragraphs[0].runs[0].font.size = Pt(DRAWN_PT)
    second = frame.add_paragraph()
    second.text = "Painted bullet item"
    presentation.save(PPTX)

    with zipfile.ZipFile(PPTX) as package:
        parts = {name: package.read(name) for name in package.namelist()}

    rels = parts[SLIDE_RELS].decode("utf-8")
    fallback_rel = next_relationship_id(rels)
    rels = rels.replace(
        "</Relationships>",
        f'<Relationship Id="{fallback_rel}" Type="{IMAGE_REL}" '
        f'Target="../media/bullet-fallback.png"/></Relationships>',
    )
    svg_rel = next_relationship_id(rels)
    rels = rels.replace(
        "</Relationships>",
        f'<Relationship Id="{svg_rel}" Type="{IMAGE_REL}" '
        f'Target="../media/bullet.svg"/></Relationships>',
    )
    raster_rel = next_relationship_id(rels)
    rels = rels.replace(
        "</Relationships>",
        f'<Relationship Id="{raster_rel}" Type="{IMAGE_REL}" '
        f'Target="../media/star.png"/></Relationships>',
    )
    parts[SLIDE_RELS] = rels.encode("utf-8")

    parts["ppt/media/bullet-fallback.png"] = fallback_png()
    parts["ppt/media/bullet.svg"] = SVG.read_bytes()
    parts["ppt/media/star.png"] = raster_png()
    declare_media_types(parts)

    drawn = (
        f'<a:buBlip><a:blip r:embed="{fallback_rel}"><a:extLst>'
        f'<a:ext uri="{SVG_EXTENSION_URI}">'
        f'<asvg:svgBlip xmlns:asvg="{ASVG}" r:embed="{svg_rel}"/>'
        f"</a:ext></a:extLst></a:blip></a:buBlip>"
    )
    painted = f'<a:buBlip><a:blip r:embed="{raster_rel}"/></a:buBlip>'
    parts[SLIDE] = with_picture_bullets(
        parts[SLIDE].decode("utf-8"), [drawn, painted]
    ).encode("utf-8")

    write_zip(PPTX, [(name, data) for name, data in parts.items()])


def main() -> None:
    write_docx()
    print(f"wrote {DOCX}")
    write_pptx()
    print(f"wrote {PPTX}")


if __name__ == "__main__":
    main()
