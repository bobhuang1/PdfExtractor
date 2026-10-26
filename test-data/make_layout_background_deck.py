#!/usr/bin/env python3
"""Author `layout_background_deck.pptx`: a deck whose photograph is behind two
slides because their *layout* states it — the middle of the inheritance chain.

A background can be stated by the slide, by its layout, or by the layout's
master, and the nearest part that states one is the one behind the page.
`master_background_deck.pptx` puts the picture in the master, where the
outermost level answers. This deck asks the question in between: the master
states one picture and the layout states another, so a slide that states neither
must take the layout's — the level nearer to it — and never the master's.

    master:  <p:bg><p:bgPr><a:blipFill>…ppt/media/master_backdrop.webp
    layout:  <p:bg><p:bgPr><a:blipFill>…ppt/media/layout_backdrop.jpeg
    slide:   nothing at all

Both are the same scan in two encodings, so a reading that reached straight
through to the master would carry the other picture, and its bytes and media
type say which level answered. python-pptx cannot author a picture fill on a
background at any level, and its master states a theme background instead
(`<p:bgRef idx="1001"><a:schemeClr val="bg1"/></p:bgRef>`), so the layout's
`p:bg` is inserted and the master's is replaced whole in the saved package, each
naming its picture through its own part's relationships.

The deck has three slides, all on the blank layout:

* the first two state no `p:bg` at all, so the photograph behind them is the
  layout's — the master's is in the package beside it, which is what makes the
  question worth asking;
* the third states a colour background of its own, which is where the
  inheritance stops, before either picture.

Each slide says a sentence, so the words in front of the backdrop are read
beside it.

Regenerate with:

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_layout_background_deck.py

The deck is written by python-pptx — a real PPTX writer — and the two
backgrounds are the pieces it cannot author, added the way PowerPoint writes
them. The tests pin what the reader makes of the package, never the container's
timestamps.
"""

from pathlib import Path
import posixpath
import zipfile

from xml.etree import ElementTree

from pptx import Presentation
from pptx.util import Inches

HERE = Path(__file__).resolve().parent
OUT = HERE / "layout_background_deck.pptx"
LAYOUT_PICTURE = HERE / "images" / "scan.jpg"
MASTER_PICTURE = HERE / "images" / "scan.webp"

MASTER = "ppt/slideMasters/slideMaster1.xml"
LAYOUT_MEDIA = "ppt/media/layout_backdrop.jpeg"
MASTER_MEDIA = "ppt/media/master_backdrop.webp"
LAYOUT_REL_ID = "rIdLayoutBackdrop"
MASTER_REL_ID = "rIdMasterBackdrop"
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
            return posixpath.normpath(
                posixpath.join(posixpath.dirname(part), target)
            )
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

    # The package declares the media type by extension; both encodings are
    # python-pptx's defaults for the photograph's own, so keep the fixture honest
    # if that ever changes.
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

    The element is opened with its name where a non-placeholder content shape
    states one (`<p:cSld name="Blank">` on a blank layout), so the opening tag
    is found rather than spelled.
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


def state_picture_background(
    parts: dict[str, bytes], part: str, media: str, rel: str, source: Path
) -> None:
    """Replace the `p:bg` `part` states with a picture fill, or add one."""
    add_picture(parts, part, media, rel, source)
    text = parts[part].decode("utf-8")
    background = BACKGROUND.format(rel=rel)
    start = text.find("<p:bg>")
    end = text.find("</p:bg>")
    if start >= 0 and end >= 0:
        parts[part] = (
            text[:start] + background + text[end + len("</p:bg>") :]
        ).encode("utf-8")
        return
    inside_c_sld(parts, part, background)


def main() -> None:
    deck = Presentation()

    for words in (
        "A photograph behind these words.",
        "The same photograph behind these words.",
        "A colour behind these words.",
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

    state_picture_background(parts, master, MASTER_MEDIA, MASTER_REL_ID, MASTER_PICTURE)
    state_picture_background(parts, layout, LAYOUT_MEDIA, LAYOUT_REL_ID, LAYOUT_PICTURE)

    if layout == master:
        raise SystemExit("the slide's layout is its master")

    # The third slide says what is behind it, so neither picture must reach it:
    # the colour is added the same way the pictures are.
    third = "ppt/slides/slide3.xml"
    if "<p:bg>" in parts[third].decode("utf-8"):
        raise SystemExit(f"{third} already has a background")
    inside_c_sld(parts, third, COLOUR)

    with zipfile.ZipFile(OUT, "w", zipfile.ZIP_DEFLATED) as package:
        for name, data in parts.items():
            package.writestr(name, data)

    print(f"wrote {OUT} ({OUT.stat().st_size} bytes); layout {layout}, master {master}")


if __name__ == "__main__":
    main()
