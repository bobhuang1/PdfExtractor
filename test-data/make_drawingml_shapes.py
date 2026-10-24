#!/usr/bin/env python3
"""Write `drawingml_shapes.docx`: shapes a Word file states in DrawingML.

A modern shape is not a picture in the package. Word states it as a `wps:wsp` in
`w:drawing` — a preset outline, a transform, and the fill and line it paints —
inside `mc:AlternateContent`, with a VML copy of the same shape in the Fallback
for readers that predate DrawingML. There is no image part behind it: Word paints
it from the markup, and a reader that only looked for an image relationship left
such a shape out of the document altogether. This package is that shape twice
over, from a writer that emits it: LibreOffice's DOCX export, with nothing
edited afterwards.

It holds, each in a paragraph of its own:

* a rounded box, `120 × 60` pt, filled `#156082` with a 2 pt `#0f4761` outline;
* a group, `100 × 50` pt, in a frame of its own — a rectangle outline and an
  oval filled `#6da33a` placed at `60,0` of the group's frame, so a child's
  transform has to be mapped through the group's `a:chOff`/`a:chExt`;
* a five-pointed star, `80 × 80` pt, filled `#4f81bd` with a 2 pt `#1f3864`
  outline;
* the words before and after, so a test can hold the text to them.

Regenerate with:

    python3 test-data/make_drawingml_shapes.py

The document is set and exported by the LibreOffice on the machine, through the
bridge in `export_shapes.py` — `python3` because the UNO bindings that bridge
speaks ship with the distribution. That is not a shortcut around the fixture's
point but the point itself: a shape composed here would be a shape only this
repository can read. LibreOffice writes the same structures Word writes
(`mc:AlternateContent`/`wps`, `wp:inline`, `a:xfrm`, `a:prstGeom`, the VML
Fallback), and the export is kept exactly as it comes out.

What that leaves out is a shape whose colour is a theme reference: LibreOffice
states an explicit `a:srgbClr` where Word writes `a:fillRef` and
`a:schemeClr`. Those shapes are in `test-data/real/shapes-with-text.docx`, a
Word file, and in the theme-reference unit tests in `crates/drawingml`.

The timestamps and shape ids are whatever LibreOffice assigns, so a regenerated
package is not byte-for-byte the committed one; the test pins what the reader
makes of the shapes, never the container.
"""

from pathlib import Path

from export_shapes import export

HERE = Path(__file__).resolve().parent
DOCX = HERE / "drawingml_shapes.docx"


def main() -> None:
    export(DOCX)


if __name__ == "__main__":
    main()
