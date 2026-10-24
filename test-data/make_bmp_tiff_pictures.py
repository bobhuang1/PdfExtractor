#!/usr/bin/env python3
"""Write `bmp_tiff_pictures.docx`, `bmp_tiff_pictures.pptx` and their pictures.

A Word or PowerPoint package can keep a picture in an encoding that is not the
PNG Docling's own office readers leave every picture in — a BMP or a TIFF. These
fixtures put both encodings in front of the office readers in a Word and in a
PowerPoint package, so a test can hold each conversion to two `image/png`
pictures at the grids their headers state, with a real PNG beside the JSON whose
bytes are the transcode of the media part, not that part renamed.

Each package carries the same two pictures in the same order, with a sentence
before and after them:

* `images/chart.bmp` — a 60 × 36 24-bit bitmap, the size in its file header;
* `images/chart.tiff` — a 40 × 24 strip, 150 dpi, the size in its first IFD.

Both encodings are written by Pillow, and both are placed by python-docx and
python-pptx as ordinary pictures, so the packages are what Word and PowerPoint
themselves would write for these formats — no part is rewritten after saving.

Regenerate with:

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_bmp_tiff_pictures.py

The libraries are used only to write the fixtures; the readers under test never
see this script, and they read the committed bytes.
"""

from pathlib import Path

from docx import Document
from docx.shared import Inches
from PIL import Image, ImageDraw
from pptx import Presentation
from pptx.util import Inches as PptxInches

HERE = Path(__file__).resolve().parent
IMAGES = HERE / "images"
BMP = IMAGES / "chart.bmp"
TIFF = IMAGES / "chart.tiff"
DOCX = HERE / "bmp_tiff_pictures.docx"
PPTX = HERE / "bmp_tiff_pictures.pptx"

BMP_SIZE = (60, 36)
TIFF_SIZE = (40, 24)
TIFF_DPI = 150


def write_pictures() -> None:
    IMAGES.mkdir(exist_ok=True)

    chart = Image.new("RGB", BMP_SIZE, (232, 238, 247))
    draw = ImageDraw.Draw(chart)
    for left, colour in [(4, "#2f6db5"), (22, "#6da33a"), (38, "#e0b23b")]:
        draw.rectangle([(left, 6), (left + 12, 29)], fill=colour)
    chart.save(BMP)

    diagram = Image.new("RGB", TIFF_SIZE, (232, 238, 247))
    draw = ImageDraw.Draw(diagram)
    draw.line([(4, 19), (14, 11), (22, 15), (36, 5)], fill="#2f6db5", width=2)
    diagram.save(TIFF, dpi=(TIFF_DPI, TIFF_DPI))


def write_docx(path: Path) -> None:
    document = Document()
    document.add_paragraph("Before the pictures.")
    document.add_picture(str(BMP), width=Inches(1.5), height=Inches(0.9))
    document.add_picture(str(TIFF), width=Inches(1.0), height=Inches(0.6))
    document.add_paragraph("After the pictures.")
    document.save(path)


def write_pptx(path: Path) -> None:
    presentation = Presentation()
    slide = presentation.slides.add_slide(presentation.slide_layouts[6])  # blank

    before = slide.shapes.add_textbox(
        PptxInches(0.8), PptxInches(0.4), PptxInches(4.0), PptxInches(0.4)
    )
    before.text_frame.text = "Before the pictures."

    slide.shapes.add_picture(
        str(BMP),
        PptxInches(0.8),
        PptxInches(1.0),
        width=PptxInches(1.5),
        height=PptxInches(0.9),
    )
    slide.shapes.add_picture(
        str(TIFF),
        PptxInches(0.8),
        PptxInches(2.2),
        width=PptxInches(1.0),
        height=PptxInches(0.6),
    )

    after = slide.shapes.add_textbox(
        PptxInches(0.8), PptxInches(3.2), PptxInches(4.0), PptxInches(0.4)
    )
    after.text_frame.text = "After the pictures."

    presentation.save(path)


def main() -> None:
    write_pictures()
    write_docx(DOCX)
    write_pptx(PPTX)
    for path in (BMP, TIFF, DOCX, PPTX):
        print(f"wrote {path} ({path.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
