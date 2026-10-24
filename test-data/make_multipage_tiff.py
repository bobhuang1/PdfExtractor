#!/usr/bin/env python3
"""Write `images/pages.tiff`: a three-page TIFF, the way a scanner writes a batch.

A TIFF is a chain of image file directories, not one picture: a scanner's feeder
saves each sheet as a page of the one file. A reader that stops at the first
directory reports one page and loses the rest, so this fixture says its own words
on every page, in type a recogniser reads without trouble, and gives the last
page a different shape — a reader that measured the first page and reused its
grid would place that page's text wrongly.

The file declares 72 dpi, so a page is its pixel grid at one point per pixel,
and the pages are written uncompressed, which is what a scanner's "none" setting
produces.

Regenerate with:

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_multipage_tiff.py

Pillow is used only to write the fixture; the reader under test never sees this
script, and it reads the committed bytes.
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

HERE = Path(__file__).resolve().parent
TIFF = HERE / "images" / "pages.tiff"

SANS_BOLD = "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"
# The words, and the sheet's own size: the last page is a different shape.
PAGES = [("PAGE ONE", 480, 160), ("PAGE TWO", 480, 160), ("PAGE THREE", 432, 192)]


def page(words: str, index: int, size: tuple[int, int]) -> Image.Image:
    image = Image.new("RGB", size, "white")
    draw = ImageDraw.Draw(image)
    draw.text((30, 26), words, font=ImageFont.truetype(SANS_BOLD, 46), fill="black")
    draw.text(
        (30, 96),
        f"Page {index} of {len(PAGES)}",
        font=ImageFont.truetype(SANS_BOLD, 22),
        fill="black",
    )
    return image


def main() -> None:
    pages = [
        page(words, index + 1, (width, height))
        for index, (words, width, height) in enumerate(PAGES)
    ]
    # `save_all` writes the later directories; `compression="raw"` keeps each
    # page's strips as data rather than deflated, which is how a scanner saves
    # a batch it means to hand to an archive.
    pages[0].save(
        TIFF,
        save_all=True,
        append_images=pages[1:],
        compression="raw",
        dpi=(72, 72),
    )
    print(f"wrote {TIFF}")


if __name__ == "__main__":
    main()
