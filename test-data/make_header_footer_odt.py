#!/usr/bin/env python3
"""Write `test-data/header_footer_odt.odt` with LibreOffice.

The third Writer fixture asks what the other two do not: a master page with a
header and a footer. The ODT reader does not read them yet, and neither does
Docling's OpenDocument backend, so the fixture and its reference pin the body
both readings hold and record that a producer's running header and footer stay
out of it on both sides.

The document is authored as flat ODF and re-saved by LibreOffice as a package,
so the fixture is what a real producer writes (the same reason
`make_notes_odt.py` writes its source flat, and `make_brief_odt.py` converts a
DOCX rather than writing a package by hand).

Run from the repository root:

    python3 test-data/make_header_footer_odt.py
"""

import pathlib
import shutil
import subprocess
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent

FLAT = """<?xml version="1.0" encoding="UTF-8"?>
<office:document
    xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
    xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0"
    xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
    xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    office:version="1.3"
    office:mimetype="application/vnd.oasis.opendocument.text">
  <office:meta>
    <dc:title>Headers and footers</dc:title>
  </office:meta>
  <office:styles/>
  <office:automatic-styles>
    <style:page-layout style:name="pm1">
      <style:page-layout-properties
          fo:page-width="21cm" fo:page-height="29.7cm"
          fo:margin-top="2cm" fo:margin-bottom="2cm"
          fo:margin-left="2cm" fo:margin-right="2cm"/>
      <style:header-style>
        <style:header-footer-properties
            fo:min-height="1cm" fo:margin-bottom="0.5cm"/>
      </style:header-style>
      <style:footer-style>
        <style:header-footer-properties
            fo:min-height="1cm" fo:margin-top="0.5cm"/>
      </style:footer-style>
    </style:page-layout>
  </office:automatic-styles>
  <office:master-styles>
    <style:master-page style:name="Standard" style:page-layout-name="pm1">
      <style:header>
        <text:p>Quarterly report — the running header</text:p>
      </style:header>
      <style:footer>
        <text:p>Confidential — the page footer</text:p>
      </style:footer>
    </style:master-page>
  </office:master-styles>
  <office:body>
    <office:text>
      <text:h text:outline-level="1">Headers and footers</text:h>
      <text:p>The body paragraph sits between the running header and the page footer.</text:p>
      <text:p>A second paragraph keeps the flow from being a single line.</text:p>
    </office:text>
  </office:body>
</office:document>
"""


def main() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        source = pathlib.Path(scratch) / "header_footer_odt.fodt"
        source.write_text(FLAT, encoding="utf-8")
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
        target = pathlib.Path(scratch) / "header_footer_odt.odt"
        if not target.is_file():
            raise SystemExit("LibreOffice wrote no header_footer_odt.odt")
        destination = ROOT / "header_footer_odt.odt"
        shutil.copy(target, destination)
    print(f"wrote {destination}")


if __name__ == "__main__":
    main()
