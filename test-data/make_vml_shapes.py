#!/usr/bin/env python3
"""Write `vml_shapes.docx`: shapes and a WordArt stated in VML.

An older Word file — or a `.doc` rewritten as DOCX by an older converter — states
its shapes in VML, in a `w:pict` group, with no image part behind them: Word
paints the shape from the markup, and no DrawingML copy exists beside it. It
holds, each in a paragraph of its own:

* a filled, rounded box, `120 × 60` pt, with a 2 pt outline;
* a group, `100 × 50` pt, in a frame of its own: a rectangle outline and an oval
  filled `#6da33a` at `60,0`, so a child is placed inside the group's
  `coordorigin`/`coordsize` rather than on the page;
* a WordArt whose string lives only in `v:textpath`, stretched across a
  `180 × 40` pt shape;
* a gradient box, a shadowed box and a line with an arrowhead, in the markup
  Word writes for each: a `v:fill type="gradient"` with a second colour and an
  angle, a `v:shadow on="t"` with an offset and an opacity, and a `v:stroke`
  `endarrow` on a `v:line` — the three ways an autoshape is more than the flat
  box a reader that stopped at `fillcolor` would paint;
* a Word horizontal rule (`o:hr`), which is a paragraph's line and not a
  picture, so the reader must leave it out;
* the words before and after, so a test can hold the text to them.

Regenerate with:

    python3 test-data/make_vml_shapes.py

Where the markup comes from matters, which is why this script does not compose
it. The box and the group are the VML Fallbacks of a LibreOffice DOCX export —
the same export `make_drawingml_shapes.py` keeps whole, with its Choice dropped
here, because that is the markup a converter emits for a file that states shapes
once. The WordArt and the rule are the two shapes Writer will not write as VML,
so they are written in Word's own form: a `v:shapetype` (`_x0000_t136`) whose
formulas the `v:shape` only refers to by `type`, and the rule as Word draws it in
`test-data/real/drawing.docx` (`o:hralign`, `o:hrstd`, `o:hr`, a zero-width
`v:rect`). Composing the shapes themselves is what made this fixture unreadable
to Word in the first place: a `v:textpath` with no shapetype behind it has no
outline to run along, and a `v:group` whose children sit outside its
`coordsize` is painted somewhere else.

The rest of the package — styles, settings, font table, theme — is the export's,
and the timestamps are fixed so a regenerated package is byte-for-byte the
committed one.
"""

from pathlib import Path
import re
import tempfile
import zipfile

from export_shapes import export

HERE = Path(__file__).resolve().parent
DOCX = HERE / "vml_shapes.docx"

# Word draws a WordArt as a `v:shape` on a shared shapetype: the type carries the
# outline (a rounded box scaled by `adj`) and the formulas its path is stated in,
# and `v:textpath` runs the string along that path.
WORDART = """<w:pict>
      <v:shapetype id="_x0000_t136" coordsize="21600,21600" o:spt="136" adj="10800"
                   path="m@7,l@8,m@5,21600l@6,21600e">
        <v:formulas>
          <v:f eqn="sum #0 0 10800"/>
          <v:f eqn="prod #0 2 1"/>
          <v:f eqn="sum 21600 0 @1"/>
          <v:f eqn="sum 0 0 @2"/>
          <v:f eqn="sum 21600 0 @3"/>
          <v:f eqn="if @0 @3 0"/>
          <v:f eqn="if @0 21600 @1"/>
          <v:f eqn="if @0 0 @2"/>
          <v:f eqn="if @0 @4 21600"/>
          <v:f eqn="mid @5 @6"/>
          <v:f eqn="mid @8 @5"/>
          <v:f eqn="mid @7 @8"/>
          <v:f eqn="mid @6 @7"/>
          <v:f eqn="sum @6 0 @5"/>
        </v:formulas>
        <v:path textpathok="t" o:connecttype="custom"
                o:connectlocs="@9,0;@10,10800;@11,21600;@12,10800"
                o:connectangles="270,180,90,0"/>
        <v:textpath on="t" fitshape="t"/>
        <o:lock v:ext="edit" text="t" shapetype="t"/>
      </v:shapetype>
      <v:shape id="WordArt 1" o:spid="_x0000_s1027" type="#_x0000_t136"
               style="position:absolute;left:0;text-align:left;margin-left:0;width:180pt;height:40pt;z-index:251659264"
               fillcolor="#c00000" stroked="f">
        <v:textpath style="font-family:&quot;Impact&quot;;font-size:36pt" string="WordArt"/>
      </v:shape>
    </w:pict>"""

# A gradient fill: two colours across the shape at an angle, which VML states
# on the `v:fill` rather than as its `fillcolor`.
GRADIENT = """<w:pict>
      <v:rect style="position:absolute;width:120pt;height:60pt"
              strokecolor="#0f4761" strokeweight="1pt">
        <v:fill type="gradient" color="#ff0000" color2="#0000ff" angle="0"/>
      </v:rect>
    </w:pict>"""

# A shadow: the shape's silhouette offset down and right and laid down under it.
SHADOW = """<w:pict>
      <v:rect style="position:absolute;width:80pt;height:40pt"
              fillcolor="#6da33a" stroked="f">
        <v:shadow on="t" color="#000000" opacity="0.5" offset="5pt,5pt"/>
      </v:rect>
    </w:pict>"""

# A connector's arrowhead, sized in multiples of the line's own width.
ARROW = """<w:pict>
      <v:line from="0,0" to="100,0" strokecolor="#1f4e79" strokeweight="2pt">
        <v:stroke endarrow="block" endarrowwidth="medium" endarrowlength="medium"/>
      </v:line>
    </w:pict>"""

# A horizontal rule as Word writes one: a zero-width `v:rect` that the reader
# paints as the paragraph's line, not as a picture.
RULE = """<w:pict>
      <v:rect id="_x0000_i1026" style="width:0;height:1.5pt" o:hralign="center"
              o:hrstd="t" o:hr="t" fillcolor="#aca899" stroked="f"/>
    </w:pict>"""


def fallback(document: str, index: int) -> str:
    """The `w:pict` of the `index`th shape the export states twice."""
    fallbacks = re.findall(
        r"<mc:Fallback><w:pict>(.*?)</w:pict></mc:Fallback>", document, re.DOTALL
    )
    return f"<w:pict>{fallbacks[index]}</w:pict>"


def vml_document(document: str) -> str:
    """The export's document with the Choice dropped and the Word-only shapes
    appended."""
    root = re.search(r"<w:document[^>]*>", document).group(0)
    section = re.search(r"<w:sectPr>.*?</w:sectPr>", document, re.DOTALL).group(0)

    def paragraph(content: str) -> str:
        return f"    <w:p><w:r>{content}</w:r></w:p>"

    def prose(written: str) -> str:
        return f"    <w:p><w:r><w:t>{written}</w:t></w:r></w:p>"

    body = [
        prose("Before the shapes."),
        paragraph(fallback(document, 0)),
        paragraph(fallback(document, 1)),
        paragraph(WORDART),
        paragraph(GRADIENT),
        paragraph(SHADOW),
        paragraph(ARROW),
        paragraph(RULE),
        prose("After the shapes."),
        f"    {section}",
    ]
    return (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
        + root
        + "\n  <w:body>\n"
        + "\n".join(body)
        + "\n  </w:body>\n</w:document>\n"
    )


def main() -> None:
    # The box and the group are what Writer draws; the export is kept only long
    # enough to take its VML.
    with tempfile.TemporaryDirectory(prefix="vml-shapes-export-") as scratch:
        source = Path(scratch) / "export.docx"
        export(source, announce=False)
        with zipfile.ZipFile(source) as package:
            parts = {name: package.read(name) for name in package.namelist()}

    document = parts["word/document.xml"].decode("utf-8")
    parts["word/document.xml"] = vml_document(document).encode("utf-8")

    # A fixed timestamp keeps a regenerated package identical to the committed
    # one, so a change in the file is a change in the fixture.
    stamp = (2020, 1, 1, 0, 0, 0)
    with zipfile.ZipFile(DOCX, "w", zipfile.ZIP_DEFLATED) as package:
        for name, data in parts.items():
            info = zipfile.ZipInfo(name, date_time=stamp)
            info.compress_type = zipfile.ZIP_DEFLATED
            package.writestr(info, data)
    print(f"wrote {DOCX} ({DOCX.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
