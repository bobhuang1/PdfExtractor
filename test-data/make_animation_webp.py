#!/usr/bin/env python3
"""Write `images/turning.webp`: a three-frame animation of three printed pages.

An animated WebP is a strip of pages as an animated GIF is: an `ANIM` chunk
states the canvas and each `ANMF` chunk paints a frame on it. A reader that
stops at the first frame reports one page and loses the other two, so this
fixture makes the loss visible in the words: each frame says its own, in large
type a recogniser reads without trouble.

The canvas is 480 x 160 pixels, and WebP declares no resolution, so every page
is that grid at the 72 dpi default — 480 x 160 points, one pixel per point. The
animation is written losslessly, so its frames are the pixels drawn here.

Regenerate with:

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_animation_webp.py

Pillow is used only to write the fixture; the reader under test never sees this
script, and it reads the committed bytes.
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

HERE = Path(__file__).resolve().parent
WEBP = HERE / "images" / "turning.webp"

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
    print(f"wrote {WEBP}")


if __name__ == "__main__":
    main()
