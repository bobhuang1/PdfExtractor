#!/usr/bin/env python3
"""Author `q3_review.pptx`, the PPTX parser's fixture.

The deck is built with python-pptx and saved beside the corpus rather than in
it, the way `inventory.xlsx` is: it is ours to license, and it is small enough
to read in full. It carries one of everything the slice reads:

* three slides, so a page is a slide and the deck's own size comes from
  `ppt/presentation.xml`;
* a title placeholder (a `title` item) and a body placeholder whose paragraphs
  are bullets by way of the slide master's `bodyStyle` — the chain a real deck
  relies on, not a bullet written into the paragraph;
* a nested bullet (`lvl="1"`) and a text box with auto-numbering written
  directly into the paragraph, so both halves of the marker resolution are
  exercised;
* a multi-run paragraph, so `txBody` runs can be joined into one line — the way
  Docling's own PPTX backend joins them;
* a `graphicFrame` table with a merged row, read with its `gridSpan`;
* a picture, reached through the slide's own relationships rather than by
  guessing a part name, so `ppt/media/` is found the way the package describes
  it;
* a chart: a `graphicFrame` that carries no picture at all, only a relationship
  to a chart part whose caches hold its title, categories and two series — one
  with a point missing, which is the gap the chart draws;
* speaker notes on the first and the last slide, the first in two paragraphs;
* comments, and the comment authors they name — two on the first slide from two
  different authors, one on the last.

The PNG is written with the standard library only: the fixture should not need
an image library to be regenerated.

The comments are the one part python-pptx cannot author (1.0.2 has no comments
API), so they are written into the saved package: `ppt/commentAuthors.xml`, a
`ppt/comments/commentN.xml` per slide that has one, the relationships from the
presentation and from the slide, and the content-type overrides a reader needs
to find them. What PowerPoint itself writes is the same four things.

    ~/.venvs/pdfextractor-docling/bin/python make_deck_pptx.py
"""

import struct
import tempfile
import zipfile
import zlib
from pathlib import Path

from pptx import Presentation
from pptx.chart.data import CategoryChartData
from pptx.enum.chart import XL_CHART_TYPE
from pptx.oxml.ns import qn
from pptx.util import Inches, Pt

OUT = Path(__file__).resolve().parent / "q3_review.pptx"

# What the deck says about itself: two people commenting on the numbers, and
# the speaker notes for the slides that need them.
COMMENT_AUTHORS = (
    '<p:cmAuthorLst xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">'
    '<p:cmAuthor id="0" name="Ada Lovelace" initials="AL" lastIdx="2" clrIdx="0"/>'
    '<p:cmAuthor id="1" name="Grace Hopper" initials="GH" lastIdx="1" clrIdx="1"/>'
    "</p:cmAuthorLst>"
)

# Slide number → the comments on it, as (author id, time, comment id, words).
COMMENTS = {
    1: [
        (0, "2026-10-18T09:15:00.000", 1, "Check the Q3 numbers against the ledger."),
        (1, "2026-10-18T10:30:00.000", 2, "Add the regional split to slide 2."),
    ],
    3: [(1, "2026-10-18T16:05:00.000", 1, "Ship this one.")],
}

NOTES = {
    1: (
        "Open with the revenue line, then the regions.\n"
        "The board asked for the unit numbers first."
    ),
    3: "Say the picture is the growth by quarter; do not read the table aloud.",
}

COMMENTS_CONTENT_TYPE = (
    "application/vnd.openxmlformats-officedocument.presentationml.comments+xml"
)
AUTHORS_CONTENT_TYPE = (
    "application/vnd.openxmlformats-officedocument.presentationml.commentAuthors+xml"
)
OFFICE_RELS = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"


def png(width: int, height: int) -> bytes:
    """A small RGB PNG: three vertical bars, one per quarter of the growth."""
    rows = []
    for y in range(height):
        row = bytearray([0])  # filter type: none
        for x in range(width):
            if x < width // 3:
                pixel = (32, 96, 160)
            elif x < 2 * width // 3:
                pixel = (64, 160, 96)
            else:
                pixel = (200, 120, 48)
            row.extend(pixel)
        # A white band across the middle, so the picture is not one flat colour.
        if height // 3 <= y < 2 * height // 3:
            for x in range(width):
                row[1 + 3 * x : 4 + 3 * x] = b"\xff\xff\xff"
        rows.append(bytes(row))
    raw = b"".join(rows)

    def chunk(kind: bytes, body: bytes) -> bytes:
        return (
            struct.pack(">I", len(body))
            + kind
            + body
            + struct.pack(">I", zlib.crc32(kind + body) & 0xFFFFFFFF)
        )

    header = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def comments_part(entries) -> str:
    """The `p:cmLst` of one slide's comments, as PowerPoint writes it."""
    body = "".join(
        f'<p:cm authorId="{author}" dt="{time}" idx="{index}">'
        f'<p:pos x="10" y="10"/><p:text>{text}</p:text></p:cm>'
        for author, time, index, text in entries
    )
    return (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
        '<p:cmLst xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">'
        f"{body}</p:cmLst>"
    )


def add_comments(path: Path) -> None:
    """Write the comment parts, their relationships and their content types
    into an already-saved package.

    python-pptx cannot author a comment, so the package is rewritten the way
    PowerPoint writes one: an authors part named from the presentation, a
    comments part per slide named from that slide, and an override for each so
    the package still says what its parts are.
    """
    with zipfile.ZipFile(path) as package:
        parts = {name: package.read(name) for name in package.namelist()}

    def text_of(name: str) -> str:
        return parts[name].decode("utf-8")

    relationships = {
        "ppt/_rels/presentation.xml.rels": (
            f"{OFFICE_RELS}/commentAuthors",
            "commentAuthors.xml",
            "rIdCommentAuthors",
        ),
        **{
            f"ppt/slides/_rels/slide{slide}.xml.rels": (
                f"{OFFICE_RELS}/comments",
                f"../comments/comment{slide}.xml",
                f"rIdComments{slide}",
            )
            for slide in COMMENTS
        },
    }
    for name, (kind, target, id) in relationships.items():
        body = text_of(name)
        assert id not in body, f"{name} already names {id}"
        parts[name] = body.replace(
            "</Relationships>",
            f'<Relationship Id="{id}" Type="{kind}" Target="{target}"/>'
            "</Relationships>",
        ).encode("utf-8")

    overrides = [("/ppt/commentAuthors.xml", AUTHORS_CONTENT_TYPE)] + [
        (f"/ppt/comments/comment{slide}.xml", COMMENTS_CONTENT_TYPE)
        for slide in COMMENTS
    ]
    types = text_of("[Content_Types].xml").replace(
        "</Types>",
        "".join(
            f'<Override PartName="{name}" ContentType="{kind}"/>'
            for name, kind in overrides
        )
        + "</Types>",
    )
    parts["[Content_Types].xml"] = types.encode("utf-8")

    parts["ppt/commentAuthors.xml"] = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
        + COMMENT_AUTHORS
    ).encode("utf-8")
    for slide, entries in COMMENTS.items():
        parts[f"ppt/comments/comment{slide}.xml"] = comments_part(entries).encode(
            "utf-8"
        )

    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as package:
        for name, data in parts.items():
            package.writestr(name, data)


def numbered(paragraph):
    """Mark a paragraph as an auto-numbered list item at level 0."""
    pPr = paragraph._p.get_or_add_pPr()
    pPr.set("lvl", "0")
    pPr.append(pPr.makeelement(qn("a:buAutoNum"), {"type": "arabicPeriod"}))


def main() -> None:
    prs = Presentation()

    # --- slide 1: a title and a body placeholder, bullets from the master ---
    slide = prs.slides.add_slide(prs.slide_layouts[1])
    slide.shapes.title.text = "Q3 Review"
    body = slide.placeholders[1].text_frame
    body.text = "Revenue grew across every region."
    for text in ["North America led with 145 units.", "Europe followed at 121 units."]:
        body.add_paragraph().text = text
    # The second line is a nested bullet: `lvl="1"` under the master's `lvl2pPr`.
    body.paragraphs[2].level = 1

    box = slide.shapes.add_textbox(Inches(0.8), Inches(5.6), Inches(6.0), Inches(0.6))
    box.text_frame.text = "Prepared for the board."

    # --- slide 2: a graphicFrame table with a merged row and a numbered list ---
    slide = prs.slides.add_slide(prs.slide_layouts[5])
    slide.shapes.title.text = "Regional performance"
    shape = slide.shapes.add_table(
        4, 3, Inches(0.8), Inches(1.8), Inches(7.0), Inches(2.4)
    )
    table = shape.table
    rows = [
        ["Region", "Q2", "Q3"],
        ["North", "120", "145"],
        ["South", "98", "121"],
        ["Total", "218", "266"],
    ]
    for row, values in enumerate(rows):
        for col, value in enumerate(values):
            table.cell(row, col).text = value
    # The total's label holds two columns: a `gridSpan="2"` cell, with the
    # covered slot written as `a:hMerge="1"` and no cell of its own.
    table.cell(3, 0).merge(table.cell(3, 1))

    box = slide.shapes.add_textbox(Inches(0.8), Inches(4.6), Inches(7.0), Inches(1.6))
    frame = box.text_frame
    frame.text = "Open the dashboard."
    numbered(frame.paragraphs[0])
    for text in ["Filter by region.", "Export the summary."]:
        paragraph = frame.add_paragraph()
        paragraph.text = text
        numbered(paragraph)

    # --- slide 3: one paragraph of several runs, and a picture ---
    slide = prs.slides.add_slide(prs.slide_layouts[5])
    slide.shapes.title.text = "Where the growth came from"

    box = slide.shapes.add_textbox(Inches(0.8), Inches(1.6), Inches(5.2), Inches(1.2))
    paragraph = box.text_frame.paragraphs[0]
    for text, bold, italic in [
        ("Revenue grew ", False, False),
        ("12%", True, False),
        (" year over year", False, False),
        (" (constant currency)", False, True),
    ]:
        run = paragraph.add_run()
        run.text = text
        run.font.bold = bold
        run.font.italic = italic
        run.font.size = Pt(20)

    # python-pptx copies the picture into the package when it is saved, so the
    # source file does not need to outlive this run.
    with tempfile.TemporaryDirectory() as scratch:
        picture = Path(scratch) / "growth.png"
        picture.write_bytes(png(160, 80))
        slide.shapes.add_picture(
            str(picture), Inches(6.2), Inches(1.6), width=Inches(2.4), height=Inches(1.2)
        )
        prs.save(OUT)

    # --- slide 3: a chart, which is data rather than a picture file ---
    data = CategoryChartData()
    data.categories = ["North America", "Europe", "Asia Pacific"]
    data.add_series("Q2", (120, 98, 74))
    # A missing point is a gap in the chart, and a gap in the reconstructed
    # table — not a plotted zero. The row keeps its category, so the gap stays.
    data.add_series("Q3", (145, 121, None))
    chart = slide.shapes.add_chart(
        XL_CHART_TYPE.COLUMN_CLUSTERED,
        Inches(0.8),
        Inches(3.4),
        Inches(8.0),
        Inches(3.2),
        data,
    )
    chart.chart.has_title = True
    chart.chart.chart_title.text_frame.text = "Units by region"

    # --- what the deck says about its slides: notes and comments ---
    for number, text in NOTES.items():
        prs.slides[number - 1].notes_slide.notes_text_frame.text = text
    prs.save(OUT)
    add_comments(OUT)
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
