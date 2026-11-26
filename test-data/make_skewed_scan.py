#!/usr/bin/env python3
"""Write `invoice_scan_skewed.png`: the scan, swept onto the platen at an angle.

The page is `invoice_scan.png` turned ten degrees clockwise about its centre by
Pillow, on a white background and the same 620 x 350, 150 dpi grid. Ten degrees
is the angle the fixture was chosen at, measured: at four and six degrees
Tesseract still reads the lines, at eight it loses two of them and at ten it
reads nothing, so the page is one `--ocr-deskew` has something to prove on.

It is the fixture the deskew estimator was validated against: the estimator
reports ten degrees for it (9.75 on the quarter-degree grid), and the deskewed
page reads the lines the turn left intact. Turning a page inside its own frame
crops the title's top-left corner — the invoice's title sits 28 pixels below
the top edge, which a ten-degree turn about the centre carries out of the page
— so the title is clipped in the stored pixels and no turn can bring it back;
the test asserts the two lines whose pixels survive.

Regenerate with:

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_skewed_scan.py
"""

from pathlib import Path

from PIL import Image

HERE = Path(__file__).resolve().parent
SOURCE = HERE / "invoice_scan.png"
TARGET = HERE / "invoice_scan_skewed.png"

#: The turn, in degrees clockwise. `Image.rotate` turns anticlockwise, so the
#: stored page runs downhill to the right, which is the sign the estimator
#: reports as positive.
DEGREES = 10


def main() -> None:
    with Image.open(SOURCE) as source:
        page = source.convert("L")
    # Bilinear: the turn is a resampling, as a scanner's would be, so the
    # estimator is not handed edges no scanner could produce.
    turned = page.rotate(-DEGREES, resample=Image.BILINEAR, fillcolor=255)
    turned.save(TARGET, dpi=(150, 150))
    print(f"wrote {TARGET} ({TARGET.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
