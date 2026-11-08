#!/usr/bin/env python3
"""Write the tiny tokenizer files the chunk tests read.

`tokenizer-mini.json` is a hand-written Hugging Face BPE model. The two files
here are the same vocabulary in the other two formats the CLI accepts:

* `tokenizer-mini.model` is a SentencePiece protobuf with a visible-space
  piece and one `▁abcdefgh` piece, so the word costs one token instead of the
  approximation's two.
* `tokenizer-mini.tiktoken` is a tiktoken rank file with every byte and the
  same whole-word token. A rank file has no split pattern; the loader supplies
  cl100k_base's pattern for a file whose name does not say o200k.

The model is written by hand because no SentencePiece trainer is installed
here, and because a fixture this small is reviewable arithmetic rather than an
opaque training artefact.

Run from the repository root:

    python3 test-data/make_tokenizer_fixtures.py
"""

from __future__ import annotations

import base64
import pathlib
import struct

ROOT = pathlib.Path(__file__).resolve().parent
SPACE = "\u2581"


def varint(value: int) -> bytes:
    out = bytearray()
    while True:
        byte = value & 0x7F
        value >>= 7
        if value:
            out.append(byte | 0x80)
        else:
            out.append(byte)
            return bytes(out)


def key(field: int, wire: int) -> bytes:
    return varint((field << 3) | wire)


def varint_field(out: bytearray, field: int, value: int) -> None:
    out.extend(key(field, 0))
    out.extend(varint(value))


def fixed32_field(out: bytearray, field: int, value: float) -> None:
    out.extend(key(field, 5))
    out.extend(struct.pack("<f", value))


def string_field(out: bytearray, field: int, value: str) -> None:
    encoded = value.encode()
    out.extend(key(field, 2))
    out.extend(varint(len(encoded)))
    out.extend(encoded)


def message_field(out: bytearray, field: int, value: bytes) -> None:
    out.extend(key(field, 2))
    out.extend(varint(len(value)))
    out.extend(value)


def piece(text: str, score: float, kind: int) -> bytes:
    out = bytearray()
    string_field(out, 1, text)
    fixed32_field(out, 2, score)
    varint_field(out, 3, kind)
    return bytes(out)


def write_sentencepiece() -> None:
    pieces = [
        piece("<unk>", 0.0, 2),  # UNKNOWN
        piece("<s>", 0.0, 3),  # CONTROL
        piece("</s>", 0.0, 3),  # CONTROL
        piece(SPACE, -5.0, 1),  # NORMAL
    ]
    pieces.extend(piece(char, -1.0, 1) for char in "abcdefgh")
    pieces.append(piece(SPACE + "abcdefgh", 0.0, 1))

    model = bytearray()
    for part in pieces:
        message_field(model, 1, part)

    trainer = bytearray()
    varint_field(trainer, 40, 0)  # unk_id
    message_field(model, 2, bytes(trainer))

    normalizer = bytearray()
    string_field(normalizer, 1, "identity")
    varint_field(normalizer, 3, 1)  # add_dummy_prefix
    varint_field(normalizer, 4, 1)  # remove_extra_whitespaces
    varint_field(normalizer, 5, 1)  # escape_whitespaces
    message_field(model, 3, bytes(normalizer))

    (ROOT / "tokenizer-mini.model").write_bytes(model)


def write_tiktoken() -> None:
    lines = []
    for rank, byte in enumerate(range(256)):
        token = base64.b64encode(bytes([byte])).decode()
        lines.append(f"{token} {rank}")
    token = base64.b64encode(b"abcdefgh").decode()
    lines.append(f"{token} 256")
    (ROOT / "tokenizer-mini.tiktoken").write_text("\n".join(lines) + "\n")


def main() -> None:
    write_sentencepiece()
    write_tiktoken()
    print("wrote tokenizer-mini.model and tokenizer-mini.tiktoken")


if __name__ == "__main__":
    main()
