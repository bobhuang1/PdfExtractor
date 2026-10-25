#!/usr/bin/env python3
"""Write two three-page TIFFs: a scanner's batch, upright and turned.

A TIFF is a chain of image file directories, not one picture: a scanner's feeder
saves each sheet as a page of the one file. A reader that stops at the first
directory reports one page and loses the rest, so `images/pages.tiff` says its
own words on every page, in type a recogniser reads without trouble, and gives
the last page a different shape — a reader that measured the first page and
reused its grid would place that page's text wrongly.

`images/pages_turned.tiff` is the same batch fed the other way up. Each
directory states its own orientation (TIFF tag 274, the EXIF tag a camera writes
for one photograph): the first sheet is stored upright and tagged 1, the second
is stored a quarter turn counter-clockwise and tagged 6 (display it turned a
quarter turn clockwise), and the third is stored the other way round and tagged
8. Every page is 480 x 160 once turned, but the second and third are stored
160 x 480 — so a reader that reused the first directory's grid, or ignored the
tags, would place their words on a sideways page and the recogniser would read
noise. The tags are asserted below rather than assumed, because a fixture that
quietly stopped stating them would test nothing.

Both files declare 72 dpi, so a page is its pixel grid at one point per pixel,
and the pages are written uncompressed, which is what a scanner's "none"
setting produces. The text is drawn upright and the pixels are turned for
storage, so a page's words are the words it was drawn with however its pixels
are stored.

Regenerate with:

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_multipage_tiff.py

Pillow is used only to write the fixtures; the readers under test never see this
script, and they read the committed bytes.
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

HERE = Path(__file__).resolve().parent
TIFF = HERE / "images" / "pages.tiff"
TURNED_TIFF = HERE / "images" / "pages_turned.tiff"

SANS_BOLD = "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"
# The words, and the sheet's own size: the last page is a different shape.
PAGES = [("PAGE ONE", 480, 160), ("PAGE TWO", 480, 160), ("PAGE THREE", 432, 192)]

# The turned batch: every page is 480 x 160 once upright, and each directory
# states the turn its stored pixels need. `Image.rotate` turns
# counter-clockwise, so tag 6 ("turn clockwise to display") is stored as a
# quarter turn counter-clockwise and tag 8 the other way round.
TURNED_PAGES = [("PAGE ONE", 1), ("PAGE TWO", 6), ("PAGE THREE", 8)]
TURNED_SIZE = (480, 160)


def page(words: str, subline: str, size: tuple[int, int]) -> Image.Image:
    image = Image.new("RGB", size, "white")
    draw = ImageDraw.Draw(image)
    draw.text((30, 26), words, font=ImageFont.truetype(SANS_BOLD, 46), fill="black")
    draw.text(
        (30, 96),
        subline,
        font=ImageFont.truetype(SANS_BOLD, 22),
        fill="black",
    )
    return image


def upright_pages() -> list[Image.Image]:
    return [
        page(words, f"Page {index + 1} of {len(PAGES)}", (width, height))
        for index, (words, width, height) in enumerate(PAGES)
    ]


def turned_pages() -> list[Image.Image]:
    """The same batch with each sheet's pixels turned and its own tag stated.

    Pillow writes the tag from the frame's `encoderinfo`, which `save_all` keeps
    per appended image; a single `tiffinfo` would tag the first directory only.
    """
    stored = []
    for index, (words, orientation) in enumerate(TURNED_PAGES):
        image = page(words, f"Sheet {index + 1} of {len(TURNED_PAGES)}", TURNED_SIZE)
        if orientation == 6:
            image = image.rotate(90, expand=True)
        elif orientation == 8:
            image = image.rotate(-90, expand=True)
        image.encoderinfo = {
            "tiffinfo": {274: orientation},
            "compression": "raw",
            "dpi": (72, 72),
        }
        stored.append(image)
    return stored


def write(path: Path, pages: list[Image.Image]) -> None:
    # `save_all` writes the later directories; `compression="raw"` keeps each
    # page's strips as data rather than deflated, which is how a scanner saves
    # a batch it means to hand to an archive.
    pages[0].save(
        path,
        save_all=True,
        append_images=pages[1:],
        compression="raw",
        dpi=(72, 72),
    )
    print(f"wrote {path}")


def main() -> None:
    write(TIFF, upright_pages())
    write(TURNED_TIFF, turned_pages())
    check_turned()


def check_turned() -> None:
    """Hold the turned fixture to what the docstring says it is.

    A reader that ignored the tags would still see three pages, so the fixture's
    own claim is the stored grid and the tag of each directory — and this fails
    the generator rather than leaving a test that reads an upright batch.
    """
    with Image.open(TURNED_TIFF) as file:
        stored = []
        for index in range(file.n_frames):
            file.seek(index)
            # `Image.size` is the upright page: Pillow applies the tag on read,
            # so the stored grid is the width/height tags themselves.
            stored.append(
                (
                    file.size,
                    (file.tag_v2[256], file.tag_v2[257]),
                    file.tag_v2.get(274),
                )
            )
    expected = [
        (TURNED_SIZE, TURNED_SIZE, 1),
        (TURNED_SIZE, (160, 480), 6),
        (TURNED_SIZE, (160, 480), 8),
    ]
    if stored != expected:
        raise SystemExit(f"{TURNED_TIFF} is not the turned batch it claims: {stored}")


if __name__ == "__main__":
    main()
