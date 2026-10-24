#!/usr/bin/env python3
"""Author the two RTF fixtures that carry a bitmap.

Both fixtures show the same small bitmap, `test-data/images/rtf_bitmap.bmp`
(written here): a 24 × 16 grid of six-pixel colour blocks, one BMP file with a
`BITMAPINFOHEADER` and 24-bit rows.

`word_bitmap.rtf` is in **Word's** shape. Word writes a bitmap into `\\pict` as
`\\wbitmap`/`\\dibitmap`: the device-independent bitmap on its own — the
`BITMAPINFOHEADER` and the pixels, with `\\picw`/`\\pich` and the `\\*goal` sizes
around it — because the RTF file, not a `BITMAPFILEHEADER`, is the container.
Nothing on this machine can write that shape: the only word processor here,
LibreOffice, re-encodes every raster as PNG on RTF export (see below). So this
one is authored to Word's format by this script, hex digit for hex digit, and
the test that reads it says so rather than pretending otherwise.

`rtf_bitmap.rtf` is **LibreOffice's** own output: a DOCX holding the same BMP is
written with python-docx and converted with `soffice --headless --convert-to
rtf`, so the fixture is a real word processor's file, control-word soup and all.
LibreOffice always spells an embedded raster `\\pngblip` — a BMP included — so
the picture carried out of this one is a PNG, which is the whole reason the
Word-shaped fixture above exists next to it.

    ~/.venvs/pdfextractor-docling/bin/python make_rtf_fixtures.py
"""

import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
BMP = HERE / "images/rtf_bitmap.bmp"
WORD = HERE / "word_bitmap.rtf"
LIBRE = HERE / "rtf_bitmap.rtf"

# Six-pixel blocks: black, red, green, blue, grey, white — every channel and a
# couple of equal-channels values, so a wrong byte order cannot pass unnoticed.
COLOURS = [
    (0x00, 0x00, 0x00),
    (0xC0, 0x20, 0x20),
    (0x20, 0xA0, 0x40),
    (0x30, 0x50, 0xC0),
    (0x80, 0x80, 0x80),
    (0xF0, 0xF0, 0xF0),
]


def pixels(width, height):
    """Row-major RGB triples, top-down, as a reader sees them."""
    return [
        [COLOURS[(x // 3) % len(COLOURS)] for x in range(width)] for _ in range(height)
    ]


def bmp_bytes(width, height):
    """A 24-bit BMP file: a `BITMAPFILEHEADER`, a `BITMAPINFOHEADER`, the rows.

    Rows are stored bottom-up and padded to four bytes, as every BMP is, so the
    fixture is a file any reader opens rather than a shape this project likes.
    """
    row = width * 3
    padding = (4 - row % 4) % 4
    dib = bytearray()
    dib += (40).to_bytes(4, "little")  # header size
    dib += width.to_bytes(4, "little", signed=True)
    dib += height.to_bytes(4, "little", signed=True)
    dib += (1).to_bytes(2, "little")  # planes
    dib += (24).to_bytes(2, "little")  # bits per pixel
    dib += (0).to_bytes(4, "little")  # BI_RGB
    dib += (0).to_bytes(4, "little")  # image size (rows are unpadded only when 0)
    dib += (2835).to_bytes(4, "little", signed=True)  # 72 dpi, pixels per metre
    dib += (2835).to_bytes(4, "little", signed=True)
    dib += (0).to_bytes(4, "little")  # palette entries used
    dib += (0).to_bytes(4, "little")  # important entries
    rows = pixels(width, height)
    for row_pixels in reversed(rows):
        for red, green, blue in row_pixels:
            dib += bytes((blue, green, red))  # BMP stores BGR
        dib += bytes(padding)

    file_header = bytearray()
    file_header += b"BM"
    file_header += (14 + len(dib)).to_bytes(4, "little")
    file_header += (0).to_bytes(4, "little")  # reserved
    # Where the pixels start in the file: past the file header and the DIB's
    # own header, which is where the rows above begin. A BMP that names 14 here
    # is read as pixels starting at the header.
    file_header += (14 + 40).to_bytes(4, "little")
    return bytes(file_header) + bytes(dib)


def hexed(data, per_line=128):
    digits = data.hex()
    return "\n".join(digits[i : i + per_line] for i in range(0, len(digits), per_line))


def word_rtf(dib, width, height):
    """Word's own spelling: `\\wbitmap0` and the DIB, with no file header.

    The controls around it are the ones Word 97 writes for a bitmap pasted into
    a document: the picture's pixel size, the size it is drawn at in twips, and
    the hex payload. The file is ANSI, one paragraph either side of the picture.
    """
    return (
        "{\\rtf1\\ansi\\ansicpg1252\\deff0\\nouicompat{\\fonttbl{\\f0\\fnil\\fcharset0 Calibri;}}\n"
        "{\\*\\generator Riched20 10.0.19041}\\viewkind4\\uc1\n"
        "\\pard\\f0\\fs22 A picture below, written the way Word writes one.\\par\n"
        "\\pard\\qc{\\pict\\wbitmap0\\picw"
        f"{width}\\pich{height}\\picwgoal{width * 15}\\pichgoal{height * 15}\n"
        f"{hexed(dib)}}}\\par\n"
        "\\pard\\ql After the picture.\\par\n"
        "}\n"
    )


def libreoffice_rtf(bmp):
    """`rtf_bitmap.rtf` as LibreOffice writes it, from a DOCX holding `bmp`."""
    try:
        import docx
    except ImportError:
        sys.exit("python-docx is needed; run this with the Docling venv's python")
    if shutil.which("soffice") is None:
        sys.exit("LibreOffice (soffice) is needed to write rtf_bitmap.rtf")

    document = docx.Document()
    document.add_heading("A word processor's bitmap", level=1)
    document.add_paragraph("The picture below is embedded as a bitmap.")
    document.add_picture(str(bmp))
    document.add_paragraph("And a paragraph after it.")

    with tempfile.TemporaryDirectory() as scratch:
        source = Path(scratch) / "rtf_bitmap.docx"
        document.save(source)
        subprocess.run(
            ["soffice", "--headless", "--convert-to", "rtf", "--outdir", scratch, str(source)],
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        shutil.copyfile(Path(scratch) / "rtf_bitmap.rtf", LIBRE)


def main():
    width, height = 24, 16
    bmp = bmp_bytes(width, height)
    BMP.write_bytes(bmp)
    WORD.write_text(word_rtf(bmp[14:], width, height), encoding="ascii")
    libreoffice_rtf(BMP)
    print(f"wrote {BMP}\nwrote {WORD}\nwrote {LIBRE}")


if __name__ == "__main__":
    main()
