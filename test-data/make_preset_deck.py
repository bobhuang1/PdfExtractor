#!/usr/bin/env python3
"""Author `preset_deck.pptx`: a slide of preset shapes the reader must draw.

PowerPoint states an autoshape as markup — a `p:spPr` naming a preset outline,
a transform and, through `p:style`, the theme's colours — and paints it when the
slide is shown. The reader draws whatever preset it knows and leaves the rest
out, so this deck holds one of each preset the painted-shape table now covers,
written by a real writer rather than by hand:

* the rounded and snipped rectangles, which treat their corners one by one;
* rings and frames, whose hole is a contour inside another;
* the mathematical symbols, the brackets and the braces;
* the stars with more points than a Word document tends to use;
* the solid shapes a rim or a front edge makes read as solid — a can, a disk,
  a cube, a bevel and a folded corner;
* the flowchart symbols, which are the polygons a diagram is made of;
* the arrows, the cloud, the heart, the moon, the sun, the smiley and the tear;
* the arrows that bend round rather than step — a curved right, left, up and
  down arrow, a U-turn, a circular arrow and a swoosh — whose band and head are
  a single outline;
* the arcs, which are a part of a circle rather than a whole one;
* and two connectors — an elbow and a curve — whose path is a line.

Every shape is placed on an 90 × 50 pt grid, eight to a row, so the pictures
come out in reading order and the test can hold each one to its own cells.

Regenerate with:

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_preset_deck.py

python-pptx writes the package — layout, master and theme included — so the
fixture is a deck a real writer produced. The ZIP timestamps are not fixed, so a
regenerated package is not byte-identical; the tests pin what the reader makes
of it, never the container.
"""

from pathlib import Path

from pptx import Presentation
from pptx.enum.shapes import MSO_CONNECTOR, MSO_SHAPE
from pptx.util import Emu, Pt

HERE = Path(__file__).resolve().parent
OUT = HERE / "preset_deck.pptx"

BLANK = 6  # the blank layout: no placeholders to inherit anything from

CELL = (Pt(90), Pt(50))
COLUMNS = 8

SHAPES = [
    # Rounded and snipped rectangles: one corner, two, or a diagonal pair.
    MSO_SHAPE.ROUND_1_RECTANGLE,
    MSO_SHAPE.ROUND_2_SAME_RECTANGLE,
    MSO_SHAPE.ROUND_2_DIAG_RECTANGLE,
    MSO_SHAPE.SNIP_1_RECTANGLE,
    MSO_SHAPE.SNIP_2_SAME_RECTANGLE,
    MSO_SHAPE.SNIP_2_DIAG_RECTANGLE,
    MSO_SHAPE.SNIP_ROUND_RECTANGLE,
    MSO_SHAPE.PLAQUE,
    # Rings, frames and bands.
    MSO_SHAPE.DONUT,
    MSO_SHAPE.NO_SYMBOL,
    MSO_SHAPE.FRAME,
    MSO_SHAPE.HALF_FRAME,
    MSO_SHAPE.CORNER,
    MSO_SHAPE.DIAGONAL_STRIPE,
    # The mathematical signs.
    MSO_SHAPE.MATH_MINUS,
    MSO_SHAPE.MATH_EQUAL,
    MSO_SHAPE.MATH_NOT_EQUAL,
    MSO_SHAPE.MATH_DIVIDE,
    MSO_SHAPE.MATH_MULTIPLY,
    # Brackets and braces.
    MSO_SHAPE.LEFT_BRACKET,
    MSO_SHAPE.RIGHT_BRACKET,
    MSO_SHAPE.LEFT_BRACE,
    MSO_SHAPE.RIGHT_BRACE,
    # The many-pointed stars.
    MSO_SHAPE.STAR_12_POINT,
    MSO_SHAPE.STAR_16_POINT,
    MSO_SHAPE.STAR_24_POINT,
    MSO_SHAPE.STAR_32_POINT,
    # Solids: the rim and the front edges are what make them read as solid.
    MSO_SHAPE.CUBE,
    MSO_SHAPE.BEVEL,
    MSO_SHAPE.FOLDED_CORNER,
    MSO_SHAPE.CAN,
    # The flowchart symbols.
    MSO_SHAPE.FLOWCHART_MAGNETIC_DISK,
    MSO_SHAPE.FLOWCHART_DOCUMENT,
    MSO_SHAPE.FLOWCHART_PUNCHED_TAPE,
    MSO_SHAPE.FLOWCHART_DELAY,
    MSO_SHAPE.FLOWCHART_MANUAL_INPUT,
    MSO_SHAPE.FLOWCHART_MANUAL_OPERATION,
    MSO_SHAPE.FLOWCHART_MERGE,
    MSO_SHAPE.FLOWCHART_EXTRACT,
    MSO_SHAPE.FLOWCHART_OFFPAGE_CONNECTOR,
    MSO_SHAPE.FLOWCHART_PREDEFINED_PROCESS,
    MSO_SHAPE.FLOWCHART_INTERNAL_STORAGE,
    MSO_SHAPE.FLOWCHART_PREPARATION,
    MSO_SHAPE.FLOWCHART_SORT,
    # Arrows.
    MSO_SHAPE.QUAD_ARROW,
    MSO_SHAPE.BENT_ARROW,
    MSO_SHAPE.BENT_UP_ARROW,
    MSO_SHAPE.NOTCHED_RIGHT_ARROW,
    # The arrows that bend round rather than step: each is a band and a head
    # stated as one outline, not a box with a triangle on its edge.
    MSO_SHAPE.CURVED_RIGHT_ARROW,
    MSO_SHAPE.CURVED_LEFT_ARROW,
    MSO_SHAPE.CURVED_UP_ARROW,
    MSO_SHAPE.CURVED_DOWN_ARROW,
    MSO_SHAPE.U_TURN_ARROW,
    MSO_SHAPE.CIRCULAR_ARROW,
    MSO_SHAPE.SWOOSH_ARROW,
    # The decorative outlines.
    MSO_SHAPE.WAVE,
    MSO_SHAPE.CLOUD,
    MSO_SHAPE.HEART,
    MSO_SHAPE.MOON,
    MSO_SHAPE.SUN,
    MSO_SHAPE.SMILEY_FACE,
    MSO_SHAPE.TEAR,
    # Parts of a circle rather than a whole one.
    MSO_SHAPE.ARC,
    MSO_SHAPE.PIE,
    MSO_SHAPE.CHORD,
    MSO_SHAPE.BLOCK_ARC,
]


def main() -> None:
    deck = Presentation()
    slide = deck.slides.add_slide(deck.slide_layouts[BLANK])

    for index, shape in enumerate(SHAPES):
        left = (index % COLUMNS) * CELL[0]
        top = (index // COLUMNS) * CELL[1]
        added = slide.shapes.add_shape(shape, Emu(left), Emu(top), Emu(CELL[0]), Emu(CELL[1]))
        # No fill and no line are stated: the colours are the `p:style`
        # python-pptx writes, which name the theme's accent1.
        added.text_frame.text = ""

    # A connector's path is a line rather than a region: the elbow's two
    # segments and the curve's elbow. They take a row of their own below the
    # presets so neither overlaps a shape it could be confused with.
    row = (len(SHAPES) + COLUMNS - 1) // COLUMNS
    elbow = slide.shapes.add_connector(
        MSO_CONNECTOR.ELBOW,
        Emu(0),
        Emu(row * CELL[1]),
        Emu(CELL[0] * 2),
        Emu(row * CELL[1] + CELL[1]),
    )
    elbow.line.width = Pt(2)
    curve = slide.shapes.add_connector(
        MSO_CONNECTOR.CURVE,
        Emu(CELL[0] * 2),
        Emu(row * CELL[1]),
        Emu(CELL[0] * 4),
        Emu(row * CELL[1] + CELL[1]),
    )
    curve.line.width = Pt(2)

    deck.save(OUT)
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
