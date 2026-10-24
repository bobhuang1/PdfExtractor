#!/usr/bin/env python3
"""Write `jpx_picture.pdf`: a page whose pictures are JPEG 2000 images.

PDF carries a JPEG 2000 as an image XObject with `/Filter /JPXDecode`, and the
stream behind that filter can be either of the two things a `.jp2` file is made
of:

* the **container** — the signature box and the `jp2h` header headers around the
  `jp2c` codestream, which is what an imaging tool writes as `.jp2`;
* the **bare codestream** — just those `jp2c` bytes, starting at the `FF 4F FF 51`
  markers, which is what a producer that already knows the grid tends to embed.

Both are legal, and both are on this one page so a reader is held to both. The
pixels are `images/scan.jp2`'s — the 620 x 350 invoice written losslessly by
OpenJPEG in `make_scan_images.py` — drawn twice at the same size, one under
`Im1` and one under `Im2`, with a sentence above and below.

The point of the fixture is what the conversion does with them: Docling cannot
name `image/jp2` in an image reference, so a picture that is still a JPEG 2000
when it leaves the reader is a box with no image. The codestream is decoded and
the page's pictures are PNGs.

Regenerate with:

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_jpx_pdf.py

The generator writes the PDF itself — a page, two image XObjects, a content
stream and an xref table — because no office library here can place a JPEG 2000
and the fixture is about the file format, not about a library. The timestamps
are its own, so a regenerated file is byte for byte the committed one.
"""

import struct
from pathlib import Path

HERE = Path(__file__).resolve().parent
JP2 = HERE / "images" / "scan.jp2"
PDF = HERE / "jpx_picture.pdf"

# The scan's own grid, and the size it is drawn at on the page.
WIDTH, HEIGHT = 620, 350
DRAW = (300, 169.4)  # 620 x 350 at about 149 dpi
PAGE = (612, 792)


def codestream(jp2: bytes) -> bytes:
    """The `jp2c` box's body: the JPEG 2000 codestream inside the container."""
    pos = 0
    while pos + 8 <= len(jp2):
        length, kind = struct.unpack(">I4s", jp2[pos : pos + 8])
        header = 8
        if length == 1:  # a 64-bit length follows the box type
            length = struct.unpack(">Q", jp2[pos + 8 : pos + 16])[0]
            header = 16
        elif length == 0:  # to the end of the file
            length = len(jp2) - pos
        if kind == b"jp2c":
            return jp2[pos + header : pos + length]
        pos += length
    raise SystemExit(f"{JP2}: no jp2c box")


def pdf(objects: list[bytes]) -> bytes:
    """Assemble the objects into a file with a classic xref table."""
    out = bytearray(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")
    offsets = []
    for number, body in enumerate(objects, start=1):
        offsets.append(len(out))
        out += f"{number} 0 obj\n".encode()
        out += body
        out += b"\nendobj\n"
    xref = len(out)
    out += f"xref\n0 {len(objects) + 1}\n".encode()
    out += b"0000000000 65535 f \n"
    for offset in offsets:
        out += f"{offset:010d} 00000 n \n".encode()
    out += (
        f"trailer\n<< /Size {len(objects) + 1} /Root 1 0 R >>\n"
        f"startxref\n{xref}\n%%EOF\n"
    ).encode()
    return bytes(out)


def image(data: bytes) -> bytes:
    """An image XObject holding a JPEG 2000 stream, in either of its two forms."""
    return (
        f"<< /Type /XObject /Subtype /Image /Width {WIDTH} /Height {HEIGHT}"
        " /ColorSpace /DeviceRGB /BitsPerComponent 8"
        f" /Filter /JPXDecode /Length {len(data)} >>\nstream\n"
    ).encode() + data + b"\nendstream"


def main() -> None:
    jp2 = JP2.read_bytes()
    container = jp2
    bare = codestream(jp2)
    if not bare.startswith(b"\xff\x4f\xff\x51"):
        raise SystemExit(f"{JP2}: the jp2c body is not a codestream")

    width, height = DRAW
    content = (
        "BT /F1 18 Tf 100 720 Td (A page of JPEG 2000 pictures.) Tj ET\n"
        f"q {width} 0 0 {height} 100 500 cm /Im1 Do Q\n"
        f"q {width} 0 0 {height} 100 260 cm /Im2 Do Q\n"
        "BT /F1 12 Tf 100 220 Td (Both are the same drawing.) Tj ET\n"
    ).encode()

    objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        (
            f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE[0]} {PAGE[1]}]"
            " /Resources << /XObject << /Im1 5 0 R /Im2 6 0 R >>"
            " /Font << /F1 7 0 R >> >> /Contents 4 0 R >>"
        ).encode(),
        f"<< /Length {len(content)} >>\nstream\n".encode() + content + b"\nendstream",
        image(container),
        image(bare),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ]
    PDF.write_bytes(pdf(objects))
    print(
        f"wrote {PDF} ({PDF.stat().st_size} bytes; "
        f"container {len(container)}, codestream {len(bare)})"
    )


if __name__ == "__main__":
    main()
