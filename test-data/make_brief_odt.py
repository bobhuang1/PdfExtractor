#!/usr/bin/env python3
"""Write `test-data/brief_odt.odt` with LibreOffice from `test-data/brief.docx`.

The fixture is a Writer document rather than a hand-written package: the reader
must be held to what a real producer writes. `brief.docx` already carries a
title, a heading, a link, two lists, a table with a row span and a column span,
and a picture, so converting it gives every block the ODT reader reads.

The file's stem is `brief_odt`, not `brief`, because `brief.docx` already uses
that stem for the legacy-document bridge, and references and the ratchet are
keyed by stem: two same-stem documents cannot both hold a reference or a row.

Run from the repository root:

    python3 test-data/make_brief_odt.py
"""

import pathlib
import shutil
import subprocess
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent


def main() -> None:
    source = ROOT / "brief.docx"
    if not source.is_file():
        raise SystemExit(f"no {source}")
    with tempfile.TemporaryDirectory() as scratch:
        subprocess.run(
            [
                "soffice",
                "--headless",
                "--convert-to",
                "odt",
                "--outdir",
                scratch,
                str(source),
            ],
            check=True,
            stdout=subprocess.DEVNULL,
        )
        target = pathlib.Path(scratch) / "brief.odt"
        if not target.is_file():
            raise SystemExit("LibreOffice wrote no brief.odt")
        shutil.copy(target, ROOT / "brief_odt.odt")
    print(f"wrote {ROOT / 'brief_odt.odt'}")


if __name__ == "__main__":
    main()
