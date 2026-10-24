#!/usr/bin/env python3
"""Write `images/turning.gif`: a three-frame animation of three printed pages.

An animated GIF is a strip of pages, not one picture: each frame is what a
reader sees for a moment, drawn on the file's logical screen. A reader that
stops at the first frame reports one page and loses the other two, so this
fixture makes the loss visible in the words: each frame says its own, in large
type a recogniser reads without trouble.

The screen is 480 x 160 pixels, and a GIF declares no resolution, so every page
is that grid at the 72 dpi default — 480 x 160 points, one pixel per point.

Regenerate with:

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_animation_gif.py

Pillow is used only to write the fixture; the reader under test never sees this
script, and it reads the committed bytes.
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

HERE = Path(__file__).resolve().parent
GIF = HERE / "images" / "turning.gif"

WIDTH, HEIGHT = 480, 160
SANS_BOLD = "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"
FRAMES = ["FRAME ONE", "FRAME TWO", "FRAME THREE"]


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
    # `save_all` writes the frames after the first; `optimize=False` keeps every
    # frame the full screen, which is how an editor writes an animation of whole
    # pages and what the reader has to composite onto.
    pages[0].save(
        GIF,
        save_all=True,
        append_images=pages[1:],
        duration=400,
        loop=0,
        optimize=False,
    )
    print(f"wrote {GIF}")


if __name__ == "__main__":
    main()
