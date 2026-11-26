#!/usr/bin/env python3
"""Write the faxed-scan fixture: the invoice with a fax line's dropped pixels.

`fax_scan.png` is `invoice_scan.png` — the same 620 x 350, 150 dpi scan — with
salt-and-pepper noise over it: isolated black pixels on the paper and isolated
white ones inside the ink, which is what a fax line leaves when it drops. It is
the fixture `--ocr-despeckle` is measured on: the noisy page is read by an OCR
test twice, once as stored and once after the pass, and the words the pass
recovers are the ones the noise cost.

The noise is placed by a fixed linear congruential generator rather than
`random`, so the fixture is the same bytes on every machine and in every Python
version, and a regenerated file is byte for byte the committed one. Two percent
of the pixels are replaced — chosen by measurement, not guessed. At one percent
Tesseract reads every line as stored, so the fixture would measure nothing; at
three the noise has landed on the glyphs themselves, and the despeckled page
still reads "Thank yau" — damage a pass over isolated pixels is not meant to
undo. At two percent the invoice and total lines are lost as stored and
recovered by the pass ("invoice, 12345" and "Tétal:Duie 42.50" become
"Invoice 12345" and "Total Due 42.50"); the title line reads as stored either
way, which is why the test asserts the two recovered lines and only that the
title survives.

Regenerate with:

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_fax_scan.py
"""

from pathlib import Path

from PIL import Image

HERE = Path(__file__).resolve().parent
SOURCE = HERE / "invoice_scan.png"
TARGET = HERE / "fax_scan.png"

#: The share of the pixels the line replaces.
DENSITY = 0.02
#: The noise's seed, fixed so the fixture is reproducible.
SEED = 20261126
#: The value a dropped line paints on paper, and the one it leaves in ink.
SALT = 0
PEPPER = 255


def faxed(image: Image.Image, density: float = DENSITY) -> Image.Image:
    """`image` with `density` of its pixels replaced, salt and pepper in turn.

    The generator is the textbook `(a * x + c) mod 2^31` one, and the upper
    bits choose the coordinates: an LCG's low bits cycle, so a coordinate taken
    from them would repeat in visible lines instead of scattering.
    """
    width, height = image.size
    out = image.convert("L")
    pixels = out.load()
    state = SEED
    for index in range(int(width * height * density)):
        state = (1103515245 * state + 12345) & 0x7FFFFFFF
        x = (state >> 8) % width
        state = (1103515245 * state + 12345) & 0x7FFFFFFF
        y = (state >> 8) % height
        pixels[x, y] = SALT if index % 2 else PEPPER
    return out


def main() -> None:
    with Image.open(SOURCE) as source:
        image = faxed(source)
    # The resolution is the scan's: the page is the same 297.6 x 168 points,
    # so a recognised line lands on the same page whether it was read from the
    # clean scan or from this one.
    image.save(TARGET, dpi=(150, 150))
    print(f"wrote {TARGET} ({TARGET.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
