#!/usr/bin/env python3
"""Write `german_scan.png`: a German invoice for the `deu` OCR model.

The same 620 x 350, 150 dpi frame as `invoice_scan.png`, so the page is 297.6 x
168 points and the boxes land on it the way the English scan's do. The words
are the ones a language model decides: `Rechnung`, `Gesamtbetrag`, the umlaut
in `für` and the `ß` in `Musterstraße` are where an English model tends to lose
the reading, which is what the fixture is for. The layout is Pillow and
DejaVu, as the English scan's is — printed text, not a photograph, so the
check is about the language rather than about image quality.

Tesseract does not ship German data with its English package. The test that
reads this page (`a_german_scan_is_read_with_the_german_model`) needs
`deu.traineddata`, from `tesseract-ocr-deu` on Debian/Ubuntu, `tesseract-lang`
on macOS/Homebrew, or a `tessdata` directory named by `TESSDATA_PREFIX` (a
released binary also looks for `tessdata/` beside itself, and only `eng` is
shipped there). Without it the test prints what is missing and skips, because
a machine without the data cannot measure the reading.

**The reading was not measured where this was written.** That machine has only
`eng` and `osd`, so what the script fixes is the page, not what Tesseract
returns from it; the words the test asserts are the German ones, and the test
itself is the measurement on a machine that has the data.

Regenerate with:

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_german_scan.py
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

HERE = Path(__file__).resolve().parent
TARGET = HERE / "german_scan.png"

#: The scan's frame and resolution, shared with `invoice_scan.png`.
WIDTH, HEIGHT = 620, 350
DPI = (150, 150)

SANS = "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"
SANS_BOLD = "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"


def invoice() -> Image.Image:
    image = Image.new("RGB", (WIDTH, HEIGHT), "white")
    draw = ImageDraw.Draw(image)
    draw.text(
        (35, 28),
        "MUSTER GMBH",
        font=ImageFont.truetype(SANS_BOLD, 26),
        fill="black",
    )
    for y, line, size in [
        (88, "Rechnung Nr. 4711", 20),
        (128, "Gesamtbetrag 1.234,56 €", 20),
        (168, "Musterstraße 7, 10115 Berlin", 16),
        (208, "Vielen Dank für Ihr Vertrauen.", 16),
    ]:
        draw.text((35, y), line, font=ImageFont.truetype(SANS, size), fill="black")
    return image


def main() -> None:
    image = invoice()
    image.save(TARGET, dpi=DPI)
    print(f"wrote {TARGET} ({TARGET.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
