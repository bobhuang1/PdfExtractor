#!/usr/bin/env python3
"""Export the Writer document the two shape fixtures are built from.

Neither fixture is a document anyone typed: both exist to hold shapes that no
image part stands behind, so the markup in them has to be markup a real writer
emits, or the reader is being tested against a shape only this repository can
produce. The shapes are therefore set in a real Writer document — through the
same LibreOffice the repository's conversion path drives — and exported as DOCX:

* `make_drawingml_shapes.py` keeps the export as it comes: a `wps:wsp` in
  `mc:AlternateContent`'s Choice, a VML copy in its Fallback, the theme in the
  package, which is the shape Word 2010 and later writes when it keeps a
  compatibility copy;
* `make_vml_shapes.py` keeps only the Fallback's `w:pict` groups, the markup a
  file carries when it has no DrawingML at all, and adds the two shapes Writer
  will not write as VML (a WordArt and a horizontal rule) in the form Word
  writes them.

Word itself is not on a machine that regenerates the fixtures, and no office
library writes a shape through the DOCX format into a document model; LibreOffice
is, and its DOCX export is Word's schema. `soffice` is started headless on a
loopback port, a UNO client fills a document and saves it as "MS Word 2007 XML".

The document holds, each in a paragraph of its own, in this order:

* the words "Before the shapes.";
* a rounded box, 120 × 60 pt, filled #156082 with a 2 pt #0F4761 outline;
* a group, 100 × 50 pt, holding a rectangle outline and an oval filled #6DA33A
  at 60,0 in the group's own frame — a child's transform has to be mapped
  through the group's `a:chOff`/`a:chExt`;
* a five-pointed star, 80 × 80 pt, filled #4F81BD with a 2 pt #1F3864 outline;
* the words "After the shapes.".

`export(path)` runs that and writes the package. The timestamps and shape ids
are whatever LibreOffice assigns, so a regenerated package is not byte-for-byte
the committed one; the fixtures' tests pin what the reader makes of the shapes,
never the container.
"""

from pathlib import Path
import os
import shutil
import socket
import subprocess
import tempfile
import time

HERE = Path(__file__).resolve().parent
SOFFICE = os.environ.get("SOFFICE", "soffice")
FILTER = "MS Word 2007 XML"
TIMEOUT = 120


def length(points: float) -> int:
    """Points as Writer states lengths: 1/100 mm."""
    return round(points * 2540 / 72)


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def start(profile: Path, port: int) -> subprocess.Popen:
    """Start a headless `soffice` that listens for UNO on `port`."""
    return subprocess.Popen(
        [
            SOFFICE,
            f"--accept=socket,host=127.0.0.1,port={port};urp;StarOffice.ComponentContext",
            f"-env:UserInstallation=file://{profile}",
            "--headless",
            "--norestore",
            "--nologo",
            "--nodefault",
            "--nolockcheck",
        ],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def connect(port: int):
    """Wait for the bridge and return the desktop, or fail after `TIMEOUT`."""
    import uno

    local = uno.getComponentContext()
    resolver = local.ServiceManager.createInstanceWithContext(
        "com.sun.star.bridge.UnoUrlResolver", local
    )
    url = f"uno:socket,host=127.0.0.1,port={port};urp;StarOffice.ComponentContext"
    deadline = time.monotonic() + TIMEOUT
    while True:
        try:
            context = resolver.resolve(url)
            return context.ServiceManager.createInstanceWithContext(
                "com.sun.star.frame.Desktop", context
            )
        except Exception:
            if time.monotonic() > deadline:
                raise RuntimeError(f"soffice did not answer on port {port}")
            time.sleep(0.25)


def property(name, value):
    from com.sun.star.beans import PropertyValue

    return PropertyValue(name, 0, value, 0)


def fill(name: str):
    """A `com.sun.star.drawing.FillStyle` value."""
    import uno

    return uno.Enum("com.sun.star.drawing.FillStyle", name)


def line(name: str):
    """A `com.sun.star.drawing.LineStyle` value."""
    import uno

    return uno.Enum("com.sun.star.drawing.LineStyle", name)


def shape(doc, kind: str, size):
    """One custom shape of a DrawingML preset kind, `size` in 1/100 mm."""
    from com.sun.star.awt import Size

    created = doc.createInstance("com.sun.star.drawing.CustomShape")
    # The geometry is a DrawingML preset name: Writer writes the same
    # `a:prstGeom` for it that Word does.
    created.CustomShapeGeometry = (property("Type", kind),)
    created.setSize(Size(*size))
    return created


def frame(drawn, size, position):
    """A shape's frame, `size` and `position` in 1/100 mm."""
    from com.sun.star.awt import Point, Size

    drawn.setSize(Size(*size))
    drawn.setPosition(Point(*position))


def main_document(doc):
    """Fill `doc` with the fixture's paragraphs and shapes."""
    # 0 is `com.sun.star.text.ControlCharacter.PARAGRAPH_BREAK`.
    PARAGRAPH_BREAK = 0

    # The document's first paragraph. `insertTextContent` anchors a shape in the
    # paragraph the cursor stands in, so each shape gets a paragraph of its own
    # as the break after it moves the cursor on.
    text = doc.getText()
    cursor = text.createTextCursor()

    def words(written):
        text.insertString(cursor, written, False)
        text.insertControlCharacter(cursor, PARAGRAPH_BREAK, False)

    def place(drawn):
        text.insertTextContent(cursor, drawn, False)
        text.insertControlCharacter(cursor, PARAGRAPH_BREAK, False)

    words("Before the shapes.")

    box = shape(doc, "round-rectangle", (length(120), length(60)))
    box.FillStyle = fill("SOLID")
    box.FillColor = 0x156082
    box.LineStyle = line("SOLID")
    box.LineColor = 0x0F4761
    box.LineWidth = length(2)
    place(box)

    # A group is anchored in the text first and filled afterwards: only then do
    # its children's frames mean anything, because inside a group a position is
    # stated in the group's own frame rather than on the page.
    group = doc.createInstance("com.sun.star.drawing.GroupShape")
    text.insertTextContent(cursor, group, False)

    outline = shape(doc, "rectangle", (length(100), length(50)))
    outline.FillStyle = fill("NONE")
    outline.LineStyle = line("SOLID")
    outline.LineColor = 0x2F6DB5
    outline.LineWidth = length(2)

    oval = shape(doc, "ellipse", (length(40), length(50)))
    oval.FillStyle = fill("SOLID")
    oval.FillColor = 0x6DA33A
    oval.LineStyle = line("NONE")

    group.add(outline)
    group.add(oval)
    frame(outline, (length(100), length(50)), (0, 0))
    frame(oval, (length(40), length(50)), (length(60), 0))
    frame(group, (length(100), length(50)), (0, 0))
    text.insertControlCharacter(cursor, PARAGRAPH_BREAK, False)

    star = shape(doc, "star5", (length(80), length(80)))
    star.FillStyle = fill("SOLID")
    star.FillColor = 0x4F81BD
    star.LineStyle = line("SOLID")
    star.LineColor = 0x1F3864
    star.LineWidth = length(2)
    place(star)

    words("After the shapes.")


def export(path: Path, announce: bool = True) -> None:
    """Write the fixture document to `path`."""
    if shutil.which(SOFFICE) is None:
        raise SystemExit(f"{SOFFICE} is not on PATH: the shape fixtures need LibreOffice")

    path = Path(path).resolve()
    path.parent.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(prefix="lo-shapes-profile-") as profile:
        port = free_port()
        office = start(Path(profile), port)
        try:
            desktop = connect(port)
            doc = desktop.loadComponentFromURL(
                "private:factory/swriter", "_blank", 0, (property("Hidden", True),)
            )
            try:
                main_document(doc)
                doc.storeToURL(
                    path.as_uri(),
                    (property("FilterName", FILTER), property("Overwrite", True)),
                )
            finally:
                doc.close(False)
            desktop.terminate()
            office.wait(timeout=30)
        finally:
            if office.poll() is None:
                office.kill()
                office.wait()

    if not path.exists() or path.stat().st_size == 0:
        raise SystemExit(f"soffice wrote no package at {path}")
    if announce:
        print(f"wrote {path} ({path.stat().st_size} bytes)")


if __name__ == "__main__":
    export(HERE / "lo_shapes.docx")
