#!/usr/bin/env python3
"""Write the fax-codec scan fixtures: `ccitt_scan.pdf`, `ccitt_g3_2d_scan.pdf`
and `jbig2_scan.pdf`.

They are all the same clean invoice as `invoice_scan.png`, binarised at 128 and
placed on a 620 x 350 page. They exist to hold the fax codecs a real scan uses
when it is not a JPEG:

* `ccitt_scan.pdf` has two pages. The first is `/CCITTFaxDecode` Group 3
  one-dimensional (`/K 0`), the second Group 4 (`/K -1`). The streams are the
  strips Pillow writes in a one-bit TIFF, lifted out of that container; the
  TIFF header is not part of the PDF stream.
* `ccitt_g3_2d_scan.pdf` has one page in Group 3 two-dimensional form (`/K 1`).
  Its strip is written by Ghostscript's `tiffg32d` device, so the two-
  dimensional path is covered by a stream a real encoder wrote rather than only
  the hand-built test bitstream.
* `jbig2_scan.pdf` has one page whose image is `/JBIG2Decode`, with the symbol
  dictionary the encoder writes separately carried in `/JBIG2Globals` — the
  shape a producer uses when several pages share a dictionary.

Regenerate with:

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_scan_codec_pdfs.py

The one-dimensional and Group 4 CCITT pages need only Pillow. The two-
dimensional CCITT page additionally needs Ghostscript (`gs`) with the `tiffg32d` device. The
JBIG2 page needs `jbig2enc` on `PATH`, or named by `JBIG2ENC`; this workspace
does not depend on an encoder at runtime, so the binary can stay outside it:

    cargo install jbig2enc --root /tmp/jbig2enc-install
    JBIG2ENC=/tmp/jbig2enc-install/bin/jbig2enc \\
        ~/.venvs/pdfextractor-docling/bin/python test-data/make_scan_codec_pdfs.py

The PDFs are written by this script rather than by an office library, as
`make_jpx_pdf.py` is: the fixture is about the stream encoding, not about a
writer's defaults. No timestamp is written, so a regenerated file is byte for
byte the committed one when the encoder version is the same.
"""

import os
import shutil
import subprocess
import tempfile
from pathlib import Path

from PIL import Image

HERE = Path(__file__).resolve().parent
PNG = HERE / "invoice_scan.png"
CCITT = HERE / "ccitt_scan.pdf"
CCITT_G3_2D = HERE / "ccitt_g3_2d_scan.pdf"
JBIG2 = HERE / "jbig2_scan.pdf"

WIDTH, HEIGHT = 620, 350
PAGE = (WIDTH, HEIGHT)


def binarised() -> Image.Image:
    """The scan at one bit per pixel, thresholded without dithering.

    Dithering would turn a white page into a field of dots that no fax or
    JBIG2 encoder should be asked to preserve; a document scan is a threshold,
    and both fixtures are made from the same thresholded pixels.
    """
    grey = Image.open(PNG).convert("L")
    return grey.point(lambda value: 255 if value >= 128 else 0).convert(
        "1", dither=Image.Dither.NONE
    )


def tiff_strip(path: Path) -> bytes:
    """The raw fax strip of a one-strip TIFF, without the TIFF header."""
    with Image.open(path) as tiff:
        offsets = tiff.tag_v2[273]
        counts = tiff.tag_v2[279]
        if len(offsets) != 1 or len(counts) != 1:
            raise SystemExit(f"{path}: expected one TIFF strip, got {len(offsets)}")
        raw = path.read_bytes()
        return raw[offsets[0] : offsets[0] + counts[0]]


def tiff_fax(image: Image.Image, compression: str) -> bytes:
    with tempfile.NamedTemporaryFile(suffix=".tif", delete=False) as handle:
        path = Path(handle.name)
    try:
        image.save(path, compression=compression, dpi=(150, 150))
        return tiff_strip(path)
    finally:
        path.unlink(missing_ok=True)


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


def stream(data: bytes) -> bytes:
    return f"<< /Length {len(data)} >>\nstream\n".encode() + data + b"\nendstream"


def ccitt_image(
    data: bytes,
    k: int,
    end_of_line: bool,
    *,
    end_of_block: bool = True,
    encoded_byte_align: bool = False,
) -> bytes:
    params = (
        f"<< /K {k} /Columns {WIDTH} /Rows {HEIGHT}"
        f" /EndOfLine {'true' if end_of_line else 'false'}"
        f" /EndOfBlock {'true' if end_of_block else 'false'}"
    )
    if encoded_byte_align:
        params += " /EncodedByteAlign true"
    params += " >>"
    return (
        f"<< /Type /XObject /Subtype /Image /Width {WIDTH} /Height {HEIGHT}"
        " /ColorSpace /DeviceGray /BitsPerComponent 1"
        f" /Filter /CCITTFaxDecode /DecodeParms {params}"
        f" /Length {len(data)} >>\nstream\n"
    ).encode() + data + b"\nendstream"


def page(image_object: int, content_object: int) -> bytes:
    return (
        f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE[0]} {PAGE[1]}]"
        f" /Resources << /XObject << /Im0 {image_object} 0 R >> >>"
        f" /Contents {content_object} 0 R >>"
    ).encode()


def content() -> bytes:
    return stream(f"q {WIDTH} 0 0 {HEIGHT} 0 0 cm /Im0 Do Q\n".encode())


def write_ccitt(image: Image.Image) -> None:
    group3 = tiff_fax(image, "group3")
    group4 = tiff_fax(image, "group4")
    body = content()
    objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>",
        page(5, 6),
        page(7, 8),
        ccitt_image(group3, 0, True),
        body,
        ccitt_image(group4, -1, False),
        body,
    ]
    CCITT.write_bytes(pdf(objects))
    print(f"wrote {CCITT} ({CCITT.stat().st_size} bytes; G3 {len(group3)}, G4 {len(group4)})")


def ghostscript_fax(image: Image.Image) -> tuple[bytes, bool]:
    encoder = shutil.which("gs")
    if not encoder:
        raise SystemExit("ccitt_g3_2d_scan.pdf needs Ghostscript (gs) with tiffg32d")
    with tempfile.TemporaryDirectory() as directory:
        directory = Path(directory)
        source = directory / "scan.pdf"
        target = directory / "scan.tif"
        image.save(source, "PDF", resolution=72.0)
        result = subprocess.run(
            [
                encoder,
                "-q",
                "-dNOPAUSE",
                "-dBATCH",
                "-sDEVICE=tiffg32d",
                "-r72",
                "-dTextAlphaBits=1",
                "-dGraphicsAlphaBits=1",
                f"-sOutputFile={target}",
                str(source),
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
        )
        if result.returncode != 0:
            raise SystemExit(
                "Ghostscript could not write the Group 3 two-dimensional page: "
                + result.stderr.decode(errors="replace")
            )
        with Image.open(target) as tiff:
            if tiff.size != (WIDTH, HEIGHT):
                raise SystemExit(
                    f"{target}: expected {WIDTH}x{HEIGHT}, got {tiff.size[0]}x{tiff.size[1]}"
                )
            if tiff.tag_v2.get(259) != 3:
                raise SystemExit(f"{target}: expected Group 3 CCITT compression")
            t4_options = int(tiff.tag_v2.get(292, 0))
            if not t4_options & 1:
                raise SystemExit(f"{target}: expected two-dimensional Group 3 coding")
            return tiff_strip(target), bool(t4_options & 4)


def write_ccitt_g3_2d(image: Image.Image) -> None:
    group3_2d, rows_are_byte_aligned = ghostscript_fax(image)
    body = content()
    objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        page(5, 4),
        body,
        ccitt_image(
            group3_2d,
            1,
            True,
            end_of_block=False,
            encoded_byte_align=rows_are_byte_aligned,
        ),
    ]
    CCITT_G3_2D.write_bytes(pdf(objects))
    print(
        f"wrote {CCITT_G3_2D} ({CCITT_G3_2D.stat().st_size} bytes; "
        f"G3 2D {len(group3_2d)})"
    )


def jbig2_fragments() -> tuple[bytes, bytes]:
    encoder = os.environ.get("JBIG2ENC") or shutil.which("jbig2enc")
    if not encoder:
        raise SystemExit(
            "jbig2_scan.pdf needs jbig2enc on PATH (or JBIG2ENC); see this file's docstring"
        )
    with tempfile.TemporaryDirectory() as directory:
        base = Path(directory) / "page"
        subprocess.run(
            [encoder, "-s", "-p", "-D", "150", "-b", str(base), str(PNG)],
            check=True,
            stdout=subprocess.DEVNULL,
        )
        page_data = Path(f"{base}.0000").read_bytes()
        globals_data = Path(f"{base}.sym").read_bytes()
    return page_data, globals_data


def write_jbig2() -> None:
    page_data, globals_data = jbig2_fragments()
    body = content()
    image = (
        f"<< /Type /XObject /Subtype /Image /Width {WIDTH} /Height {HEIGHT}"
        " /ColorSpace /DeviceGray /BitsPerComponent 1"
        " /Filter /JBIG2Decode /DecodeParms << /JBIG2Globals 6 0 R >>"
        f" /Length {len(page_data)} >>\nstream\n"
    ).encode() + page_data + b"\nendstream"
    objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        page(5, 4),
        body,
        image,
        stream(globals_data),
    ]
    JBIG2.write_bytes(pdf(objects))
    print(
        f"wrote {JBIG2} ({JBIG2.stat().st_size} bytes; "
        f"page {len(page_data)}, globals {len(globals_data)})"
    )


def main() -> None:
    image = binarised()
    write_ccitt(image)
    write_ccitt_g3_2d(image)
    write_jbig2()


if __name__ == "__main__":
    main()
