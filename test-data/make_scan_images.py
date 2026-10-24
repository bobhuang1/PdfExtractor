#!/usr/bin/env python3
"""Write the plain-image fixtures: one scan, in seven encodings.

An image input is one page, and the page's size is the pixel grid divided by
the resolution the file declares. The fixture is therefore a clean, printed
invoice at a known size — 620 x 350 pixels — saved through Pillow at 150 dpi.
Each encoding declares that resolution in its own way, which is what the
reader's header parsers are held to:

* PNG writes a `pHYs` chunk in pixels per metre;
* JPEG writes a JFIF APP0 density in dots per inch;
* TIFF writes `XResolution`/`YResolution` rationals and `ResolutionUnit`;
* BMP writes pixels per metre in its DIB header;
* WebP has no resolution field of its own, so it is the 72 dpi default —
  one pixel per point — exactly as Docling's own backend reads it;
* GIF has none either: its logical screen descriptor states the grid and a
  pixel aspect ratio, not a density, so it is the same 72 dpi default. The
  reader reads it as one page, and the first frame is the pixels;
* JPEG 2000 is written losslessly by OpenJPEG, so its pixels are the PNG's,
  and like WebP Pillow states no resolution in it — a JP2 says so in a capture
  resolution box (`resc`) this writer does not emit, so the file is one pixel
  per point, exactly as the WebP is. It is both a picture inside a document,
  where it is decoded and re-encoded as PNG because Docling cannot name
  `image/jp2`, and a page on its own: the JP2 container's `ihdr` box states the
  620 x 350 grid.

`scan_150dpi.jp2` is the same codestream with the capture resolution box a
scanner's writer adds: 5906 samples per metre on both axes, which is the 150 dpi
the other encodings state. Nothing but the header boxes changes, so its pixels
are `scan.jp2`'s and its page is 297.6 x 168 points rather than 620 x 350. The
box is written here because Pillow has no option for it.

`scan.j2c` is the eighth file: the same codestream copied out of the JP2 without
the container around it, which is what a scanner writing a bare JPEG 2000 emits.
Its `SIZ` marker states the 620 x 350 grid, so its page is that grid — and one
pixel per point, because a codestream states no density: the `res` box that
places a JP2 is a container box, and copying the codestream out leaves it
behind.

`scan.jpc` is that same codestream under the other name scanners give it, and
`scan.jpf` is the 150 dpi JP2 container under a name that codestreams also use.
The pair is deliberate: which header a JPEG 2000 has is the bytes` answer, never
the extension's. A reader that trusted `.jpf` to be a codestream would miss the
capture resolution box and place the page at 620 x 350 pixels instead of
297.6 x 168 points.

`invoice_scan.png` is the fixture the OCR end-to-end test and the real Docling
reference use; the others live in `images/` and are held to the same page facts
by the parser's fixture test — WebP and `scan.jp2` to one pixel per point, and
`scan_150dpi.jp2` to the resolution its own header declares.

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_scan_images.py

Pillow is used only to write the fixture; the parser itself reads the headers,
not the pixels.
"""

import struct
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

HERE = Path(__file__).resolve().parent
WIDTH, HEIGHT = 620, 350
DPI = (150, 150)

SANS = "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"
SANS_BOLD = "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"


def invoice() -> Image.Image:
    image = Image.new("RGB", (WIDTH, HEIGHT), "white")
    draw = ImageDraw.Draw(image)
    draw.text((35, 28), "ACME SUPPLIES", font=ImageFont.truetype(SANS_BOLD, 26), fill="black")
    draw.text((35, 88), "Invoice 12345", font=ImageFont.truetype(SANS, 17), fill="black")
    draw.text((35, 128), "Total Due 42.50", font=ImageFont.truetype(SANS, 17), fill="black")
    draw.text(
        (35, 168),
        "Thank you for your business.",
        font=ImageFont.truetype(SANS, 14),
        fill="black",
    )
    return image


def with_capture_resolution(jp2: bytes, dpi: int) -> bytes:
    """The same JP2, its header declaring `dpi` on both axes.

    The box a scanner writes: a `res` superbox holding a `resc` box of the
    vertical numerator and denominator, the horizontal pair and a signed
    base-ten exponent for each. The unit is samples per metre, so 150 dpi is
    5906 of them. It is appended to `jp2h`, which the standard allows — only
    `ihdr` has to come first — and the codestream is untouched.
    """
    samples = round(dpi / 0.0254)
    resc = struct.pack(">HHHHbb", samples, 1, samples, 1, 0, 0)
    res = struct.pack(">I4s", len(resc) + 8, b"resc") + resc
    box = struct.pack(">I4s", len(res) + 8, b"res ") + res

    out = bytearray()
    pos = 0
    while pos + 8 <= len(jp2):
        length, kind = struct.unpack(">I4s", jp2[pos : pos + 8])
        header = 8
        if length == 1:
            length = struct.unpack(">Q", jp2[pos + 8 : pos + 16])[0]
            header = 16
        elif length == 0:
            length = len(jp2) - pos
        body = jp2[pos + header : pos + length]
        if kind == b"jp2h":
            out += struct.pack(">I4s", len(body) + len(box) + header, kind)
            if header == 16:
                out += struct.pack(">Q", len(body) + len(box) + header)
            out += body + box
        else:
            out += jp2[pos : pos + length]
        pos += length
    return bytes(out)


def jp2_codestream(jp2: bytes) -> bytes:
    """The codestream a JP2 wraps: its `jp2c` box's payload, byte for byte.

    A bare codestream is the picture without the container — no signature box,
    no `ftyp`, no `jp2h`, so no `ihdr` and no `resc` either. Its markers state
    the grid in `SIZ`, and nothing in a codestream states a density.
    """
    pos = 0
    while pos + 8 <= len(jp2):
        length, kind = struct.unpack(">I4s", jp2[pos : pos + 8])
        header = 8
        if length == 1:
            length = struct.unpack(">Q", jp2[pos + 8 : pos + 16])[0]
            header = 16
        elif length == 0:
            length = len(jp2) - pos
        if kind == b"jp2c":
            return jp2[pos + header : pos + length]
        pos += length
    raise ValueError("the JP2 carries no jp2c box")


def main() -> None:
    image = invoice()
    png = HERE / "invoice_scan.png"
    image.save(png, dpi=DPI)
    print(f"wrote {png}")

    images = HERE / "images"
    images.mkdir(exist_ok=True)
    for name, options in [
        ("scan.jpg", {"quality": 95, "dpi": DPI}),
        ("scan.tiff", {"dpi": DPI, "compression": "tiff_lzw"}),
        ("scan.bmp", {"dpi": DPI}),
        # WebP stores no resolution; the reader must fall back to 72 dpi.
        ("scan.webp", {"quality": 95}),
        # Lossless JPEG 2000: the pixels must be the PNG's when transcoded.
        ("scan.jp2", {"irreversible": False}),
        # GIF: no resolution field, so the screen grid at the 72 dpi default.
        ("scan.gif", {}),
    ]:
        path = images / name
        image.save(path, **options)
        print(f"wrote {path}")

    # The same lossless scan, its header stating the resolution Pillow cannot.
    # The pixels are `scan.jp2`'s, so a reader that decodes both agrees.
    source = (images / "scan.jp2").read_bytes()
    declared = images / "scan_150dpi.jp2"
    declared.write_bytes(with_capture_resolution(source, DPI[0]))
    print(f"wrote {declared}")

    # The same JPEG 2000 without its container: the `.j2c` a scanner may write,
    # whose `SIZ` marker states the grid and which declares no density.
    bare = images / "scan.j2c"
    bare.write_bytes(jp2_codestream(source))
    print(f"wrote {bare}")

    # That codestream under its other name, and the container under a name
    # codestreams also use: the bytes, not the extension, decide the header.
    jpc = images / "scan.jpc"
    jpc.write_bytes(bare.read_bytes())
    print(f"wrote {jpc}")
    jpf = images / "scan.jpf"
    jpf.write_bytes(declared.read_bytes())
    print(f"wrote {jpf}")


if __name__ == "__main__":
    main()
