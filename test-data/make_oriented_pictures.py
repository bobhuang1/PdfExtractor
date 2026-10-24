#!/usr/bin/env python3
"""Write oriented_picture.docx and oriented_picture.pptx.

Both packages carry the same photographed page as an embedded picture:
`photos/06_rotated.jpg`, the invoice whose pixels were stored 90° clockwise
from the page and whose EXIF orientation tag (6) says to turn them back. Word
and PowerPoint display that picture upright, because they honour the tag; the
package keeps the stored pixels.

That gap is the point of these fixtures: the readers must turn the picture
the way the tag says before it leaves the parser, or the image written beside
the JSON is the sideways one no reader was ever shown. The picture is placed
in an upright A4-shaped box (3.5 × 4.95 in), which is how it was inserted.

Regenerate with:

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_oriented_pictures.py

The office libraries are used only to write the fixtures; the readers under
test never see this script. The embedded bytes are the JPEG as written; neither
library re-encodes it.
"""

from pathlib import Path

from docx import Document
from docx.shared import Inches
from pptx import Presentation
from pptx.util import Inches as PptxInches

HERE = Path(__file__).resolve().parent
PICTURE = HERE / "photos" / "06_rotated.jpg"
WIDTH = Inches(3.5)
HEIGHT = Inches(4.95)


def write_docx(path: Path) -> None:
    document = Document()
    document.add_paragraph("A picture stored by a camera held sideways")
    document.add_picture(str(PICTURE), width=WIDTH, height=HEIGHT)
    document.save(path)


def write_pptx(path: Path) -> None:
    presentation = Presentation()
    slide = presentation.slides.add_slide(presentation.slide_layouts[6])  # blank
    slide.shapes.add_picture(
        str(PICTURE),
        PptxInches(1),
        PptxInches(1),
        width=PptxInches(3.5),
        height=PptxInches(4.95),
    )
    presentation.save(path)


def main() -> None:
    if not PICTURE.is_file():
        raise SystemExit(f"missing {PICTURE}; run make_photo_corpus.py first")
    docx_path = HERE / "oriented_picture.docx"
    pptx_path = HERE / "oriented_picture.pptx"
    write_docx(docx_path)
    write_pptx(pptx_path)
    print(f"wrote {docx_path} ({docx_path.stat().st_size} bytes)")
    print(f"wrote {pptx_path} ({pptx_path.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
