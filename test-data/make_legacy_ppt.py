#!/usr/bin/env python3
"""Write `test-data/legacy_deck.ppt`, the legacy-presentation bridge's fixture.

A binary `.ppt` is a compound file full of a record stream whose only public
description is reverse engineering, so there is nothing here to author by hand
the way `make_deck_pptx.py` authors OOXML. The fixture is instead the deck we
own, written by a real PowerPoint 97 writer: LibreOffice's Impress filter. That
also makes the fixture the honest case for the bridge — a legacy file that a
real application produced, converted back by the importer the extractor itself
uses — and it keeps the fixture's licence ours.

    python test-data/make_legacy_ppt.py

Needs LibreOffice (`soffice` on `PATH`, or `PDFE_SOFFICE` naming it), the same
dependency the bridge itself has. The source deck is `q3_review.pptx`; the
conversion is deterministic apart from the timestamps PowerPoint's container
records.

The deck's speaker notes and comments survive LibreOffice's PowerPoint writer,
so the fixture exercises the notes layer through the bridge, not only through
the native PPTX reader. Regenerate it whenever the source deck gains remarks;
going through this script is what keeps the two formats saying the same thing.
"""

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
SOURCE = HERE / "q3_review.pptx"
TARGET = HERE / "legacy_deck.ppt"


def soffice() -> str:
    explicit = os.environ.get("PDFE_SOFFICE")
    if explicit:
        return explicit
    found = shutil.which("soffice") or shutil.which("libreoffice")
    if not found:
        sys.exit("no soffice on PATH and PDFE_SOFFICE is not set")
    return found


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="pdfe-legacy-ppt-") as scratch:
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
                str(SOURCE),
            ],
            capture_output=True,
            text=True,
        )
        produced = out / f"{SOURCE.stem}.ppt"
        if result.returncode != 0 or not produced.is_file():
            sys.exit(
                "LibreOffice could not write the fixture: "
                f"{result.stderr.strip() or result.stdout.strip() or 'no diagnostic'}"
            )
        shutil.copyfile(produced, TARGET)
    print(f"wrote {TARGET}")


if __name__ == "__main__":
    main()
