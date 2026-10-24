#!/usr/bin/env python3
"""Write the photographed-document corpus, with its ground truth.

A scan of a page is clean; a photograph of one is not. The page is turned a few
degrees (the camera was not square to the paper), the light falls unevenly
across it (a window on one side, a shadow in a corner), the hand is not quite
steady, and the encoder adds JPEG artifacts. Those are the conditions an OCR
reading has to survive, and they are what this corpus is for.

Each file is one page of a synthetic document rendered at 150 dpi on
1240 x 1754 pixels (A4), then transformed:

* `01_flat.png` — the control: square to the frame, even light, sharp;
* `02_skew.png` — 2.5° clockwise, even light, sharp;
* `03_backlit.png` — 4° counter-clockwise, a diagonal fall-off from bright to
  dim, as if the lamp were behind the page on one side;
* `04_shadow.png` — 3°, a vignette and a heavier corner shadow, slightly out of
  focus (a little camera shake), smaller body text;
* `05_photo.jpg` — 4°, fall-off and vignette, a little softer still, smaller
  body text, JPEG at quality 72;
* `06_rotated.jpg` — the invoice square to the frame and evenly lit, but the
  camera was held sideways: the pixels are stored 90° counter-clockwise and
  EXIF orientation 6 says to turn them 90° clockwise to read the page;
* `07_sideways.jpg` — the invoice stored 90° counter-clockwise with **no EXIF
  tag at all**, as a scanner or an editor that strips metadata leaves it. The
  pixels are the only statement of orientation, so the page has to be read to
  find which way up it is;
* `08_upside_down.png` — the library hours stored upside down, again with no
  tag: the other way a page arrives the wrong way round.

The frame is cropped to the page rather than expanded to the desk around it:
the desk would add large, dim triangles that the reader is right to pick words
out of, and the corpus is about reading the page, not the table it lies on. The
turn is still visible in the text: every line is tilted.

The last two pages are deliberately at the edge of what the pipeline reads
well. They are where a change to OCR, layout or the image path would show up
as a lower score rather than as a missing word in a clean page.

The text of every page is written to `photos/corpus.json` beside the images:
the recognised text is scored against exactly those lines by
`crates/cli/tests/ocr_quality_test.rs`. Ground truth lives with the images so
the two cannot drift apart.

Everything is deterministic — the lighting masks are computed rather than
sampled, and Pillow's own rotation and blur are — so regenerating the corpus is
a no-op and a diff in the images is a real change to it.

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_photo_corpus.py

Pillow is used only to write the fixtures; the reader and OCR engine under test
never see this script.
"""

import json
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter, ImageFont

HERE = Path(__file__).resolve().parent
OUT = HERE / "photos"
DPI = 150
PAGE = (1240, 1754)  # A4 at 150 dpi

SANS = "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"
SANS_BOLD = "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"

# The pages. A heading and a handful of body lines each: enough words to give
# the accuracy a denominator, few enough to see in one look.
DOCUMENTS = {
    "invoice": [
        "ACME SUPPLIES",
        "Invoice 12345",
        "Date: 18 October 2026",
        "Bill to: Northwind Traders",
        "Total Due 42.50",
        "Thank you for your business.",
    ],
    "library": [
        "CITY LIBRARY",
        "Opening hours",
        "Monday to Friday 9:00 to 18:00",
        "Saturday 10:00 to 16:00",
        "Closed on public holidays",
        "Please return books on time.",
    ],
    "logistics": [
        "HARBOUR LOGISTICS",
        "Memo to all staff",
        "The warehouse closes at 17:30 today.",
        "Safety briefing in room 12 at 14:00.",
        "Wear your badge in the yard.",
        "Sign in at the gate before entry.",
    ],
}


def page(lines: list[str], body: int = 30) -> Image.Image:
    """A page of text: the first line a heading, the rest body text."""
    image = Image.new("RGB", PAGE, "white")
    draw = ImageDraw.Draw(image)
    y = 170
    for index, text in enumerate(lines):
        heading = index == 0
        font = ImageFont.truetype(SANS_BOLD if heading else SANS, 46 if heading else body)
        draw.text((120, y), text, font=font, fill=(25, 25, 25))
        y += 100 if heading else int(body * 2.2)
    return image


def lighting_mask(size: tuple[int, int], kind: str) -> Image.Image:
    """A grey mask to multiply the page by: 255 is fully lit, lower is dim.

    The mask is computed on a small grid and enlarged smoothly, which is both
    fast and free of the banding a 1-pixel-per-row gradient can show.
    """
    width, height = size
    grid = 96
    mask = Image.new("L", (grid, grid))
    pixels = mask.load()
    for gy in range(grid):
        for gx in range(grid):
            x = gx / (grid - 1)
            y = gy / (grid - 1)
            # Distance from the middle, normalised so the corners are 1.0.
            edge = min(1.0, ((x - 0.5) ** 2 + (y - 0.5) ** 2) ** 0.5 / 0.7071)
            if kind == "even":
                value = 255
            elif kind == "gradient":
                # Bright at the top-left, dimming towards the bottom-right.
                value = 255 - 55 * (0.65 * x + 0.35 * y)
            elif kind == "shadow":
                # A vignette, plus a heavier fall-off in the bottom-right corner,
                # as if the camera were shading the page with its own body.
                corner = max(0.0, (x + y) / 2 - 0.55) / 0.45
                value = 255 - 30 * edge - 45 * corner
            elif kind == "gradient-shadow":
                corner = max(0.0, (x + y) / 2 - 0.6) / 0.4
                value = 255 - 45 * (0.65 * x + 0.35 * y) - 25 * edge - 25 * corner
            else:
                raise ValueError(f"unknown lighting {kind!r}")
            # Keep the darkest corner readable: a photograph of a page is
            # unevenly lit, not a silhouette, and speckles in a black corner
            # are noise the reader is meant to recognise, not invent around.
            pixels[gx, gy] = int(max(150, min(255, value)))
    return mask.resize(size, Image.Resampling.BICUBIC)


def photograph(
    image: Image.Image,
    *,
    angle: float,
    lighting: str,
    blur: float = 0.0,
) -> Image.Image:
    """Turn a page into a photograph: rotate, light it, soften it."""
    turned = image.rotate(
        angle,
        resample=Image.Resampling.BICUBIC,
        expand=False,
        fillcolor=(252, 252, 250),
    )
    mask = lighting_mask(turned.size, lighting)
    turned = ImageChops.multiply(turned, Image.merge("RGB", (mask, mask, mask)))
    if blur > 0.0:
        turned = turned.filter(ImageFilter.GaussianBlur(blur))
    return turned


# (file, page, body size, skew, lighting, blur, encoding, storage)
#
# `storage` is how the page arrives: upright; turned with an EXIF orientation 6
# tag that says to turn it back; turned with no tag at all; or upside down with
# no tag. The last two are found from the pixels alone.
CASES = [
    ("01_flat.png", "invoice", 30, 0.0, "even", 0.0, "PNG", "upright"),
    ("02_skew.png", "library", 30, 2.5, "even", 0.0, "PNG", "upright"),
    ("03_backlit.png", "logistics", 30, -4.0, "gradient", 0.0, "PNG", "upright"),
    ("04_shadow.png", "invoice", 26, 3.0, "shadow", 0.8, "PNG", "upright"),
    ("05_photo.jpg", "library", 28, 4.0, "gradient-shadow", 0.9, "JPEG", "upright"),
    ("06_rotated.jpg", "invoice", 30, 0.0, "even", 0.0, "JPEG", "exif6"),
    ("07_sideways.jpg", "invoice", 30, 0.0, "even", 0.0, "JPEG", "sideways"),
    ("08_upside_down.png", "library", 30, 0.0, "even", 0.0, "PNG", "upside-down"),
]


def store_sideways(picture: Image.Image) -> Image.Image:
    """Lay the pixels down the way a camera with EXIF orientation 6 does.

    Orientation 6 is the common one: the phone was turned a quarter turn
    clockwise, so the page is stored a quarter turn counter-clockwise, and a
    viewer that honours the tag turns it back.
    """
    return picture.rotate(
        90,
        resample=Image.Resampling.BICUBIC,
        expand=True,
        fillcolor=(252, 252, 250),
    )


def store_upside_down(picture: Image.Image) -> Image.Image:
    """Lay the page down a half turn round, as an upside-down capture does."""
    return picture.rotate(
        180,
        resample=Image.Resampling.BICUBIC,
        expand=True,
        fillcolor=(252, 252, 250),
    )


def main() -> None:
    OUT.mkdir(exist_ok=True)
    documents = []
    for name, source, body, angle, lighting, blur, encoding, storage in CASES:
        lines = DOCUMENTS[source]
        picture = photograph(
            page(lines, body),
            angle=angle,
            lighting=lighting,
            blur=blur,
        )
        if storage in ("exif6", "sideways"):
            picture = store_sideways(picture)
        elif storage == "upside-down":
            picture = store_upside_down(picture)
        elif storage != "upright":
            raise ValueError(f"{name}: unknown storage {storage!r}")
        orientation = 6 if storage == "exif6" else (None if storage != "upright" else 1)
        path = OUT / name
        if encoding == "JPEG":
            options = {
                "format": "JPEG",
                "quality": 72 if storage == "upright" else 90,
                "dpi": (DPI, DPI),
            }
            if orientation == 6:
                exif = Image.Exif()
                exif[0x0112] = orientation
                options["exif"] = exif
            picture.save(path, **options)
        else:
            picture.save(path, format="PNG", dpi=(DPI, DPI))
        documents.append(
            {
                "file": name,
                "source": source,
                "condition": f"{angle:+g}° · {lighting}"
                + (f" · blur {blur:g}" if blur else "")
                + (" · JPEG 72" if encoding == "JPEG" and storage == "upright" else "")
                + (" · stored sideways · EXIF 6" if storage == "exif6" else "")
                + (" · stored sideways · no EXIF tag" if storage == "sideways" else "")
                + (" · stored upside down · no EXIF tag" if storage == "upside-down" else ""),
                "skew_degrees": angle,
                "lighting": lighting,
                "exif_orientation": orientation,
                "lines": lines,
            }
        )
        print(f"wrote {path} ({picture.width}x{picture.height})")

    manifest = OUT / "corpus.json"
    manifest.write_text(
        json.dumps(
            {
                "note": "Generated by test-data/make_photo_corpus.py; "
                "`lines` are the page's ground truth.",
                "dpi": DPI,
                "documents": documents,
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    print(f"wrote {manifest}")


if __name__ == "__main__":
    main()
