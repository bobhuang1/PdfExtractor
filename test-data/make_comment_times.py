#!/usr/bin/env python3
"""Write `test-data/comment_times.pptx` and `test-data/comment_times.ppt`.

The pair exists for one thing: the times of its comments carry non-zero
milliseconds, so the notes text a bridged deck produces can be held to the
native one's at the precision the notes layer writes. `comment_timestamp` in
the CLI cuts a comment's fraction to PowerPoint's three digits, and the bridge
makes that cut necessary: Impress writes the same instant with nine digits
(`09:15:12.125000000`). A fixture whose milliseconds are all zero cannot tell a
cut that keeps the digits from one that writes zeroes, because zeroes are all
there is; this one can.

The native deck is authored here with python-pptx and the comment parts are
written into the saved package the way PowerPoint writes them (python-pptx has
no comments API). The bridge fixture is that deck converted to `.ppt` by
Impress, then corrected in one place:

* **The millisecond field.** The binary comment record (`Comment10`) stores the
  time as year, month, day, day of week, hour, minute, second and
  **millisecond**; PowerPoint writes the milliseconds it was given. Impress'
  *writer* zeroes that field (and can round the second), while its *reader*
  keeps whatever is there — so the converted fixture is walked back to the
  source deck's instants, which is what a PowerPoint-written `.ppt` would say.
  Without this, no `.ppt` in existence could hold a non-zero sub-second time and
  the bridge's truncation could not be proven end to end.
* Nothing else is touched: the shapes, the words, the comment text and the
  authors are Impress' own, exactly as the ordinary bridge fixture produces
  them.

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_comment_times.py

Needs LibreOffice (`soffice` on `PATH`, or `PDFE_SOFFICE` naming it), the same
dependency the bridge itself has. The source deck is written first, so the pair
is always regenerated together.
"""

import os
import shutil
import struct
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

from pptx import Presentation

HERE = Path(__file__).resolve().parent
NATIVE = HERE / "comment_times.pptx"
BRIDGED = HERE / "comment_times.ppt"

# Slide -> (author id, time, comment id, words). The times are the point: two
# different milliseconds, one before and one after the second's halfway mark.
COMMENTS = [
    (0, "2026-10-18T09:15:12.125", 1, "Check the regional split before the board sees it."),
    (1, "2026-10-18T16:05:42.750", 2, "The millisecond is the point of this fixture."),
]

COMMENT_AUTHORS = (
    '<p:cmAuthorLst xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">'
    '<p:cmAuthor id="0" name="Ada Lovelace" initials="AL" lastIdx="2" clrIdx="0"/>'
    '<p:cmAuthor id="1" name="Grace Hopper" initials="GH" lastIdx="1" clrIdx="1"/>'
    "</p:cmAuthorLst>"
)

COMMENTS_CONTENT_TYPE = (
    "application/vnd.openxmlformats-officedocument.presentationml.comments+xml"
)
AUTHORS_CONTENT_TYPE = (
    "application/vnd.openxmlformats-officedocument.presentationml.commentAuthors+xml"
)
OFFICE_RELS = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"


def soffice() -> str:
    explicit = os.environ.get("PDFE_SOFFICE")
    if explicit:
        return explicit
    found = shutil.which("soffice") or shutil.which("libreoffice")
    if not found:
        sys.exit("no soffice on PATH and PDFE_SOFFICE is not set")
    return found


def comments_part(entries) -> str:
    """The `p:cmLst` of the slide's comments, as PowerPoint writes it."""
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
    """Write the comment authors, the slide's comments, their relationships and
    their content types into an already-saved package."""
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
        "ppt/slides/_rels/slide1.xml.rels": (
            f"{OFFICE_RELS}/comments",
            "../comments/comment1.xml",
            "rIdComments1",
        ),
    }
    for name, (kind, target, id) in relationships.items():
        body = text_of(name)
        assert id not in body, f"{name} already names {id}"
        parts[name] = body.replace(
            "</Relationships>",
            f'<Relationship Id="{id}" Type="{kind}" Target="{target}"/>'
            "</Relationships>",
        ).encode("utf-8")

    overrides = [
        ("/ppt/commentAuthors.xml", AUTHORS_CONTENT_TYPE),
        ("/ppt/comments/comment1.xml", COMMENTS_CONTENT_TYPE),
    ]
    parts["[Content_Types].xml"] = text_of("[Content_Types].xml").replace(
        "</Types>",
        "".join(
            f'<Override PartName="{name}" ContentType="{kind}"/>'
            for name, kind in overrides
        )
        + "</Types>",
    ).encode("utf-8")

    parts["ppt/commentAuthors.xml"] = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n' + COMMENT_AUTHORS
    ).encode("utf-8")
    parts["ppt/comments/comment1.xml"] = comments_part(COMMENTS).encode("utf-8")

    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as package:
        for name, data in parts.items():
            package.writestr(name, data)


def timestamp_parts(dt: str):
    """`2026-10-18T09:15:12.125` -> (2026, 10, 18, 9, 15, 12, 125)."""
    date, clock = dt.split("T")
    year, month, day = (int(part) for part in date.split("-"))
    hour, minute, rest = clock.split(":")
    second, fraction = rest.split(".")
    return year, month, day, int(hour), int(minute), int(second), int(fraction)


def restore_milliseconds(data: bytearray, entries) -> None:
    """Put the source deck's seconds and milliseconds back into the `.ppt`.

    A comment's date in the binary format is eight little-endian 16-bit fields:
    year, month, day, day of week, hour, minute, second, millisecond. Impress
    writes the seconds rounded and the milliseconds zero. Each comment is a
    record holding its author, its words (UTF-16LE, in a `CString`), the
    author's initials and then the date, so the date is found a hundred-odd
    bytes after the comment's own (unique) words and checked against the year,
    month, day, hour and minute it claims to be before the second and
    millisecond are rewritten.
    """
    for _author, dt, _index, text in entries:
        year, month, day, hour, minute, second, milliseconds = timestamp_parts(dt)
        at = data.find(text.encode("utf-16-le"))
        assert at >= 0, f"the converted deck does not hold {text!r}"
        assert data.find(text.encode("utf-16-le"), at + 2) < 0, (
            f"the converted deck holds {text!r} twice"
        )
        pattern = struct.pack("<HH", year, month)
        position = -1
        search = at
        while True:
            candidate = data.find(pattern, search, at + 256)
            if candidate < 0:
                break
            fields = struct.unpack_from("<8H", data, candidate)
            if fields[2] == day and fields[4] == hour and fields[5] == minute:
                assert position < 0, f"two date records claim {dt}"
                position = candidate
            search = candidate + 2
        assert position >= 0, f"no date record for {dt} before {text!r}"
        # Only the second and the millisecond move; the rest already say the
        # right day and hour, and Impress' own day-of-week field is left alone.
        struct.pack_into("<HH", data, position + 12, second, milliseconds)


def main() -> None:
    prs = Presentation()
    slide = prs.slides.add_slide(prs.slide_layouts[1])
    slide.shapes.title.text = "Comment times"
    body = slide.placeholders[1].text_frame
    body.text = "The times of its comments carry milliseconds."
    body.add_paragraph().text = "A bridge must keep them, at the precision the notes write."
    prs.save(NATIVE)
    add_comments(NATIVE)

    with tempfile.TemporaryDirectory(prefix="pdfe-comment-times-") as scratch:
        out = Path(scratch) / "out"
        out.mkdir()
        profile = Path(scratch) / "profile"
        result = subprocess.run(
            [
                soffice(),
                "--headless",
                "--norestore",
                f"-env:UserInstallation=file://{profile}",
                "--convert-to",
                "ppt",
                "--outdir",
                str(out),
                str(NATIVE),
            ],
            capture_output=True,
            text=True,
        )
        produced = out / f"{NATIVE.stem}.ppt"
        if result.returncode != 0 or not produced.is_file():
            sys.exit(
                "LibreOffice could not write the fixture: "
                f"{result.stderr.strip() or result.stdout.strip() or 'no diagnostic'}"
            )
        data = bytearray(produced.read_bytes())

    restore_milliseconds(data, COMMENTS)
    BRIDGED.write_bytes(data)
    print(f"wrote {NATIVE} and {BRIDGED}")


if __name__ == "__main__":
    main()
