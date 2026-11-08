#!/usr/bin/env python3
"""Write `test-data/q3_review_impress.odp` with LibreOffice from `q3_review.pptx`.

The file's stem is `q3_review_impress`, not `q3_review`, so it can sit beside
the deck it was converted from: the Docling references and the corpus ratchet
are keyed by stem, and a document that took the PPTX fixture's stem would be
measured against the PPTX reading instead of being read as itself.

The fixture is a real Impress package, not a hand-written OpenDocument file:
its slides carry the title/outline placeholders, a table, a picture and
speaker notes that a producer actually writes. Converting the existing PPTX
keeps the ODP deck and the PPTX deck the same content, so their readings can be
compared without inventing a second document.

Run from the repository root:

    python3 test-data/make_q3_review_odp.py
"""

import pathlib
import shutil
import subprocess
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent


def main() -> None:
    source = ROOT / "q3_review.pptx"
    if not source.is_file():
        raise SystemExit(f"no {source}")
    with tempfile.TemporaryDirectory() as scratch:
        subprocess.run(
            [
                "soffice",
                "--headless",
                "--convert-to",
                "odp",
                "--outdir",
                scratch,
                str(source),
            ],
            check=True,
            stdout=subprocess.DEVNULL,
        )
        target = pathlib.Path(scratch) / "q3_review.odp"
        if not target.is_file():
            raise SystemExit("LibreOffice wrote no q3_review.odp")
        shutil.copy(target, ROOT / "q3_review_impress.odp")
    print(f"wrote {ROOT / 'q3_review_impress.odp'}")


if __name__ == "__main__":
    main()
