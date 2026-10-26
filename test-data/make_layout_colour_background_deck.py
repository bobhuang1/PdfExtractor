#!/usr/bin/env python3
"""Author `layout_colour_background_deck.pptx`: a deck whose master states a
photograph and whose *layout* states a colour, so the picture never reaches the
slides beneath that layout.

A background can be stated by the slide, by its layout, or by the layout's
master, and the nearest part that states one is the one behind the page — but
only the *background* is taken from that part, not the parts behind it. A
colour, a gradient or a theme fill is a background and not a picture, so a
layout that states one hides the master's photograph rather than passing it
through. `master_background_deck.pptx` shows the same stop at the slide's own
`p:bg`; `layout_background_deck.pptx` shows a layout stating a picture of its
own. This deck asks the question in between, one level down from the slide:

    master:  <p:bg><p:bgPr><a:blipFill>…ppt/media/master_backdrop.webp
    layout:  <p:bg><p:bgPr><a:solidFill>…accent2
    slide:   nothing at all

The deck has three slides, all on the blank layout:

* the first two state no `p:bg` at all, so what is behind them is the layout's
  colour — the master's photograph is in the package beside it, which is what
  makes the question worth asking;
* the third states a picture background of its own, which is the nearest level
  of all: the reader still finds a backdrop where one is stated, so the bare
  pages above are the layout's doing rather than a reading that lost pictures.

python-pptx cannot author a background at any level, so all three are pieces it
writes: the master's `p:bg` is replaced whole with the photograph, the layout's
colour is inserted as the first child of its `p:cSld`, and the third slide's
`p:bg` is inserted the same way, each picture named through its own part's
relationships. Which parts answer is read off the package the writer produced —
slide 1's layout and that layout's master — rather than assumed from its
numbering.

Each slide says a sentence, so the words in front of the backdrop are read
beside it.

Regenerate with:

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_layout_colour_background_deck.py

The tests pin what the reader makes of the package, never the container's
timestamps.
"""

from pathlib import Path
import posixpath
import zipfile

from xml.etree import ElementTree

from pptx import Presentation
from pptx.util import Inches

HERE = Path(__file__).resolve().parent
OUT = HERE / "layout_colour_background_deck.pptx"
MASTER_PICTURE = HERE / "images" / "scan.webp"
SLIDE_PICTURE = HERE / "images" / "scan.jpg"

MASTER_MEDIA = "ppt/media/master_backdrop.webp"
SLIDE_MEDIA = "ppt/media/slide_backdrop.jpeg"
MASTER_REL_ID = "rIdMasterBackdrop"
SLIDE_REL_ID = "rIdSlideBackdrop"
REL_TYPE = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image"
REL_NS = "{http://schemas.openxmlformats.org/package/2006/relationships}"

BACKGROUND = (
    "<p:bg><p:bgPr>"
    '<a:blipFill rotWithShape="1"><a:blip r:embed="{rel}"/>'
    "<a:stretch><a:fillRect/></a:stretch></a:blipFill>"
    "<a:effectLst/>"
    "</p:bgPr></p:bg>"
)
COLOUR = (
    "<p:bg><p:bgPr>"
    '<a:solidFill><a:schemeClr val="accent2"/></a:solidFill>'
    "<a:effectLst/>"
    "</p:bgPr></p:bg>"
)
BG_END = "</p:bg>"


def rels_part(part: str) -> str:
    """Where the relationships of `part` live."""
    folder, name = posixpath.split(part)
    return posixpath.join(folder, "_rels", f"{name}.rels")


def target_of(parts: dict[str, bytes], part: str, kind: str) -> str:
    """The part `part` names as its `kind` relationship (`/slideLayout`, …)."""
    rels = rels_part(part)
    if rels not in parts:
        raise SystemExit(f"{part} has no {rels}")
    root = ElementTree.fromstring(parts[rels])
    for relationship in root.iter(f"{REL_NS}Relationship"):
        if relationship.get("Type", "").endswith(kind):
            if relationship.get("TargetMode") == "External":
                break
            target = relationship.get("Target", "")
            return posixpath.normpath(posixpath.join(posixpath.dirname(part), target))
    raise SystemExit(f"{rels} names no {kind} relationship")


def add_picture(parts: dict[str, bytes], part: str, media: str, rel: str, source: Path) -> None:
    """Give `part` a relationship to `media`, and the package the part itself."""
    if media in parts:
        raise SystemExit(f"{media} is already in the deck")
    rels = rels_part(part)
    text = parts[rels].decode("utf-8")
    if rel in text:
        raise SystemExit(f"{rels} already names {rel}")
    parts[rels] = text.replace(
        "</Relationships>",
        f'<Relationship Id="{rel}" Type="{REL_TYPE}" '
        f'Target="{posixpath.relpath(media, posixpath.dirname(part))}"/>'
        "</Relationships>",
    ).encode("utf-8")
    parts[media] = source.read_bytes()

    # The package declares the media type by extension, so the fixture states it
    # the way PowerPoint would if the writer ever stopped doing it.
    extension = posixpath.splitext(media)[1].lstrip(".").lower()
    stated = {"jpeg": "image/jpeg", "webp": "image/webp"}[extension]
    types = parts["[Content_Types].xml"].decode("utf-8")
    if f'Extension="{extension}"' not in types:
        types = types.replace(
            "</Types>",
            f'<Default Extension="{extension}" ContentType="{stated}"/></Types>',
        )
        parts["[Content_Types].xml"] = types.encode("utf-8")


def inside_c_sld(parts: dict[str, bytes], part: str, fragment: str) -> None:
    """Insert `fragment` as the first child of `part`'s `p:cSld`.

    The element is opened with its name where a part states one
    (`<p:cSld name="Blank">` on a blank layout), so the opening tag is found
    rather than spelled.
    """
    text = parts[part].decode("utf-8")
    start = text.find("<p:cSld")
    if start < 0:
        raise SystemExit(f"{part} has no p:cSld")
    opening = text.find(">", start)
    if opening < 0:
        raise SystemExit(f"{part}'s p:cSld is not closed")
    at = opening + 1
    parts[part] = (text[:at] + fragment + text[at:]).encode("utf-8")


def state_background(parts: dict[str, bytes], part: str, fragment: str) -> None:
    """Replace the `p:bg` `part` states with `fragment`, or add one."""
    text = parts[part].decode("utf-8")
    start = text.find("<p:bg>")
    end = text.find(BG_END)
    if start >= 0 and end >= 0:
        parts[part] = (text[:start] + fragment + text[end + len(BG_END) :]).encode(
            "utf-8"
        )
        return
    inside_c_sld(parts, part, fragment)


def state_picture_background(
    parts: dict[str, bytes], part: str, media: str, rel: str, source: Path
) -> None:
    """Replace the `p:bg` `part` states with a picture fill, or add one."""
    add_picture(parts, part, media, rel, source)
    state_background(parts, part, BACKGROUND.format(rel=rel))


def main() -> None:
    deck = Presentation()

    for words in (
        "A colour behind these words.",
        "The same colour behind these words.",
        "A picture behind these words.",
    ):
        # The blank layout, index 6 in python-pptx's default: no placeholders, so
        # each slide's own tree holds only its sentence.
        slide = deck.slides.add_slide(deck.slide_layouts[6])
        box = slide.shapes.add_textbox(
            Inches(0.8), Inches(0.6), Inches(6.0), Inches(0.6)
        )
        box.text_frame.text = words

    deck.save(OUT)

    with zipfile.ZipFile(OUT) as package:
        parts = {name: package.read(name) for name in package.namelist()}

    # Which parts answer is read off the package the writer produced rather than
    # assumed from its numbering: slide 1's layout, and that layout's master.
    layout = target_of(parts, "ppt/slides/slide1.xml", "/slideLayout")
    master = target_of(parts, layout, "/slideMaster")
    if layout == master:
        raise SystemExit("the slide's layout is its master")

    state_picture_background(parts, master, MASTER_MEDIA, MASTER_REL_ID, MASTER_PICTURE)
    # A colour is a background and not a picture: it hides the master's rather
    # than passing it through.
    state_background(parts, layout, COLOUR)
    # The third slide states a picture of its own, the nearest level of all, so
    # the reading still finds a backdrop where one is stated.
    state_picture_background(
        parts, "ppt/slides/slide3.xml", SLIDE_MEDIA, SLIDE_REL_ID, SLIDE_PICTURE
    )

    with zipfile.ZipFile(OUT, "w", zipfile.ZIP_DEFLATED) as package:
        for name, data in parts.items():
            package.writestr(name, data)

    print(f"wrote {OUT} ({OUT.stat().st_size} bytes); layout {layout}, master {master}")


if __name__ == "__main__":
    main()
