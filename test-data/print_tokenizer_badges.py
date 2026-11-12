#!/usr/bin/env python3
"""Print the tokenizer badge label for every valid fixture.

The window's `--selftest-tokenizer` path fills the tokenizer field, asks the
real `tokenizer_format` command, and reports what the badge says. This script
drives that path for the three committed fixtures and for the o200k/p50k
tiktoken copies the badge test makes, so the rendered labels can be compared
by eye without opening the window.

The o200k and p50k cases are copies of `tokenizer-mini.tiktoken`: a rank file
has no split pattern, so the loader chooses the family from the file name.

Run from anywhere:

    python3 test-data/print_tokenizer_badges.py
"""

from __future__ import annotations

import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
UI = ROOT / "target" / "debug" / "pdfextractor-ui"
FIXTURES = ROOT / "test-data"


def build_ui() -> None:
    """Build the window so the labels come from the current frontend."""
    subprocess.run(
        ["cargo", "build", "-p", "pdfextractor-ui", "--locked"],
        cwd=ROOT,
        check=True,
        stdout=subprocess.DEVNULL,
    )


def ui_command(tokenizer: pathlib.Path) -> list[str]:
    command = [
        str(UI),
        "--selftest",
        "--selftest-tokenizer",
        str(tokenizer),
    ]
    if os.environ.get("DISPLAY"):
        return command
    xvfb = shutil.which("xvfb-run")
    if xvfb is None:
        raise RuntimeError(
            "no DISPLAY and no xvfb-run: the window cannot be opened to render the badge"
        )
    return [xvfb, "-a", *command]


def label_for(tokenizer: pathlib.Path) -> str:
    result = subprocess.run(
        ui_command(tokenizer),
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
        timeout=60,
    )
    report = next(
        (
            line
            for line in result.stdout.splitlines()
            if line.startswith("selftest-rendered ")
        ),
        None,
    )
    if report is None:
        raise RuntimeError(
            f"the window rendered no badge report for {tokenizer}\n"
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    match = re.search(r'tokenizer-format="([^"]*)"', report)
    if match is None:
        raise RuntimeError(f"the badge report held no label for {tokenizer}:\n{report}")
    if 'tokenizer-format-bad=0' not in report:
        raise RuntimeError(f"the badge marked a valid tokenizer as bad:\n{report}")
    return match.group(1)


def main() -> int:
    build_ui()

    cases: list[tuple[str, pathlib.Path]] = [
        ("tokenizer-mini.json", FIXTURES / "tokenizer-mini.json"),
        ("tokenizer-mini.model", FIXTURES / "tokenizer-mini.model"),
        ("tokenizer-mini.tiktoken", FIXTURES / "tokenizer-mini.tiktoken"),
    ]

    with tempfile.TemporaryDirectory(prefix="pdfextractor-tokenizer-badges-") as temp:
        scratch = pathlib.Path(temp)
        tiktoken = FIXTURES / "tokenizer-mini.tiktoken"
        for name in ("tokenizer-mini-o200k.tiktoken", "tokenizer-mini-p50k.tiktoken"):
            path = scratch / name
            shutil.copyfile(tiktoken, path)
            cases.append((name, path))

        failed = False
        for name, path in cases:
            try:
                print(f"{name}: {label_for(path)}")
            except (OSError, RuntimeError, subprocess.SubprocessError) as error:
                failed = True
                print(f"{name}: ERROR: {error}", file=sys.stderr)
        return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
