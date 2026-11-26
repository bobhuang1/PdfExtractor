#!/usr/bin/env python3
"""Write `invoice_scan_skewed.png`: the scan, swept onto the platen at an angle.

The page is `invoice_scan.png` shrunk to 85 percent inside its 620 x 350, 150 dpi
frame and then turned ten degrees clockwise about the frame's centre by Pillow,
on a white background. The shrink is what keeps the whole page of text in the
picture: the scan's title sits 34 pixels below the top edge, and a ten-degree
turn about the centre carries that corner out of a frame the text already
fills, so the stored pixels would lose the title itself. Shrunk first, every
line stays inside the frame — the turned page's ink clears the edges by
seventeen pixels at the top and more elsewhere — and the fixture is a page
turned on a platen with margin around it, as a scan of a sheet smaller than the
glass really is.

Ten degrees is the fixture's angle, and the pass is measured on it. Read as
stored, every one of the four lines comes back wrong: `ACME SUPPLIES` as
`ACME SUPPLigg`, `Invoice 12345` as `IMvoice 12345`, `Total Due 42.50` as
`Total Due 42 50` and `Thank you for your business.` as
`TPMK You for your business,`. Read with `--ocr-deskew`, all four come back
exactly, which is what the test asserts. The turn is about the frame's centre
and keeps the grid, so the recognised boxes are mapped back onto the scan's own
297.6 x 168 points.

Regenerate with:

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_skewed_scan.py
"""

from pathlib import Path

from PIL import Image

HERE = Path(__file__).resolve().parent
SOURCE = HERE / "invoice_scan.png"
TARGET = HERE / "invoice_scan_skewed.png"

#: How much of the frame the scan fills before it is turned. Chosen so the
#: turned page keeps its whole text inside the frame: at 0.9 the title's corner
#: clears the top edge by eight pixels, at 0.87 by fourteen, and at 0.85 by
#: seventeen, while the text is still large enough for Tesseract to read it
#: once the page is turned flat.
SCALE = 0.85

#: The turn, in degrees clockwise. `Image.rotate` turns anticlockwise, so the
#: stored page runs downhill to the right, which is the sign the estimator
#: reports as positive.
DEGREES = 10


def main() -> None:
    with Image.open(SOURCE) as source:
        page = source.convert("L")
    width, height = page.size

    # The scan inside a margin: the page scaled down and placed at the centre of
    # the frame it was scanned in. Bilinear, as a scanner's resampling is.
    shrunk = page.resize(
        (round(width * SCALE), round(height * SCALE)), Image.BILINEAR
    )
    framed = Image.new("L", (width, height), 255)
    framed.paste(shrunk, ((width - shrunk.width) // 2, (height - shrunk.height) // 2))

    # Bilinear again: the turn is a resampling too, so the estimator is not
    # handed edges no scanner could produce.
    turned = framed.rotate(-DEGREES, resample=Image.BILINEAR, fillcolor=255)
    turned.save(TARGET, dpi=(150, 150))
    print(f"wrote {TARGET} ({TARGET.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
