#!/usr/bin/env python3
"""Hold a directory of our documents to docling-core itself.

This is the authoritative check on the format: the library that defines
DoclingDocument decides whether our output is a document at all. It needs
`docling-core` installed, which the Rust test suite does not, so the in-repo
golden test does the schema work and calls this script only when pointed at a
Python interpreter that has it:

    DOCLING_PY=~/.venvs/pdfextractor-docling/bin/python \\
        cargo test -p pdfextractor-cli --test docling_golden_test

Run it by hand against any output tree:

    python validate_with_docling.py path/to/output
"""

import sys
from pathlib import Path

from docling_core.types.doc import DoclingDocument


def check(path: Path) -> str | None:
    """Return a description of what is wrong, or None when the file is sound."""
    text = path.read_text(encoding="utf-8")
    try:
        # This is the check that counts: Docling's own model validator, which
        # rejects unknown fields, bad enum spellings and missing required
        # fields, and then walks the tree from `body` and `furniture`.
        document = DoclingDocument.model_validate_json(text)
    except Exception as error:  # pydantic ValidationError, or a JSON error
        detail = str(error).splitlines()
        return f"{type(error).__name__}: {detail[0] if detail else error}"
    try:
        # The schema cannot express "children and parents agree", so say it
        # again out loud: this is the failure that bug 45 was.
        document.validate_tree(document.body, raise_on_error=True)
        if document.furniture is not None:
            document.validate_tree(document.furniture, raise_on_error=True)
    except Exception as error:
        return f"validate_tree: {error}"
    return None


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    root = Path(argv[1])
    paths = sorted(path for path in root.rglob("*.json"))
    if not paths:
        print(f"no JSON documents under {root}", file=sys.stderr)
        return 2

    failed = 0
    for path in paths:
        problem = check(path)
        if problem is None:
            print(f"ok   {path}")
        else:
            failed += 1
            print(f"BAD  {path}\n     {problem}")

    print(f"\n{len(paths) - failed}/{len(paths)} document(s) accepted by docling-core")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
