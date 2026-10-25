#!/usr/bin/env python3
"""Write `images/turning.webp` and `images/turning_turned.webp`.

An animated WebP is a strip of pages as an animated GIF is: an `ANIM` chunk
states the canvas and each `ANMF` chunk paints a frame on it. A reader that
stops at the first frame reports one page and loses the other two, so this
fixture makes the loss visible in the words: each frame says its own, in large
type a recogniser reads without trouble.

The canvas is 480 x 160 pixels, and WebP declares no resolution, so every page
is that grid at the 72 dpi default — 480 x 160 points, one pixel per point. The
animation is written losslessly, so its frames are the pixels drawn here.

The same three pages are written a second time the way a phone held on its side
writes them. Every sheet is drawn upright, then stored a quarter turn
anticlockwise on a 160 x 480 canvas, and the file carries an EXIF orientation
tag (274) of 6: the stored pixels are on their side, and a reader turns them
90° clockwise to see the page. An EXIF tag speaks for the whole file, not for
its first frame, so every page is 480 x 160 once the turn is applied. A reader
that applied the tag to the first frame alone would report one upright page and
two on their side, with their words unreadable, which is what the test holds.

Regenerate with:

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_animation_webp.py

Pillow is used only to write the fixtures; the reader under test never sees this
script, and it reads the committed bytes.
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

HERE = Path(__file__).resolve().parent
WEBP = HERE / "images" / "turning.webp"
# The same three pages stored on their side, with the tag that turns them back.
TURNED = HERE / "images" / "turning_turned.webp"

WIDTH, HEIGHT = 480, 160
SANS_BOLD = "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"
FRAMES = ["WEBP ONE", "WEBP TWO", "WEBP THREE"]


def frame(words: str, index: int) -> Image.Image:
    image = Image.new("RGB", (WIDTH, HEIGHT), "white")
    draw = ImageDraw.Draw(image)
    draw.text((30, 26), words, font=ImageFont.truetype(SANS_BOLD, 46), fill="black")
    draw.text(
        (30, 96),
        f"Page {index} of {len(FRAMES)}",
        font=ImageFont.truetype(SANS_BOLD, 22),
        fill="black",
    )
    return image


def exif_orientation(value: int) -> bytes:
    """An EXIF payload whose only tag is the orientation.

    WebP's `EXIF` chunk holds an Exif payload: the `Exif\\0\\0` prefix some
    writers add, then a little-endian TIFF directory. One entry (274, SHORT,
    count 1, the value) is all a reader needs to know which way the pixels go.
    """
    header = b"Exif\x00\x00II*\x00" + (8).to_bytes(4, "little")
    directory = (1).to_bytes(2, "little")
    entry = (
        (274).to_bytes(2, "little")
        + (3).to_bytes(2, "little")
        + (1).to_bytes(4, "little")
        + value.to_bytes(2, "little")
        + b"\x00\x00"
    )
    return header + directory + entry + (0).to_bytes(4, "little")


def main() -> None:
    pages = [frame(words, index + 1) for index, words in enumerate(FRAMES)]
    # `save_all` writes the frames after the first. Lossless keeps each frame
    # the page drawn here rather than a quantisation of it; the frames are whole
    # pages, so each one replaces the canvas rather than repainting a corner.
    pages[0].save(
        WEBP,
        save_all=True,
        append_images=pages[1:],
        duration=400,
        loop=0,
        lossless=True,
    )

    # The same strip as a sideways camera stores it: Pillow's `ROTATE_90` is a
    # quarter turn anticlockwise, which is what a stored page with tag 6 is.
    turned = [page.transpose(Image.Transpose.ROTATE_90) for page in pages]
    turned[0].save(
        TURNED,
        save_all=True,
        append_images=turned[1:],
        duration=400,
        loop=0,
        lossless=True,
        exif=exif_orientation(6),
    )

    print(f"wrote {WEBP}")
    print(f"wrote {TURNED}")


if __name__ == "__main__":
    main()
