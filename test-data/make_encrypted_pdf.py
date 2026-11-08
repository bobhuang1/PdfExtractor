#!/usr/bin/env python3
"""Write `test-data/encrypted.pdf`.

A real standard-security PDF — RC4 40-bit, revision 2 — with an empty *user*
password and a non-empty owner password, so a reader opens it without asking
while its strings and streams are still ciphered. Written by hand rather than
by a library because none is installed here, and by hand the fixture is a few
kilobytes of reviewable arithmetic: the `O` and `U` entries, the file key and
the per-object keys are the ones ISO 32000-1 §7.6.3 specifies, and `pdftotext`
reading the page back proves the file is a document a real reader accepts.

The fixture exists so the CLI's refusal of an encrypted file is tested against
a file a writer could really produce, not against a plain document with an
`/Encrypt` string typed into its trailer.
"""

import hashlib
import struct
from pathlib import Path

# The 32-byte padding string of ISO 32000-1, algorithm 2.
PAD = bytes(
    [
        0x28, 0xBF, 0x4E, 0x5E, 0x4E, 0x75, 0x8A, 0x41,
        0x64, 0x00, 0x4E, 0x56, 0xFF, 0xFA, 0x01, 0x08,
        0x2E, 0x2E, 0x00, 0xB6, 0xD0, 0x68, 0x3E, 0x80,
        0x2F, 0x0C, 0xA9, 0xFE, 0x64, 0x53, 0x69, 0x7A,
    ]
)


def rc4(key: bytes, data: bytes) -> bytes:
    state = list(range(256))
    j = 0
    for i in range(256):
        j = (j + state[i] + key[i % len(key)]) & 0xFF
        state[i], state[j] = state[j], state[i]
    out = bytearray()
    i = j = 0
    for byte in data:
        i = (i + 1) & 0xFF
        j = (j + state[i]) & 0xFF
        state[i], state[j] = state[j], state[i]
        out.append(byte ^ state[(state[i] + state[j]) & 0xFF])
    return bytes(out)


def pad(password: bytes, length: int = 32) -> bytes:
    return (password + PAD)[:length]


owner = pad(b"owner")
user = pad(b"")

# Algorithm 3: O is the user padding ciphered under a key from the owner password.
o_key = hashlib.md5(owner).digest()[:5]
O = rc4(o_key, user)

# -3904 allows printing, copying, changing and annotating, and leaves the
# document unencrypted for metadata: the most open standard permission set.
P = -3904
file_id = hashlib.md5(b"pdfextractor encrypted fixture").digest()

# Algorithm 2: the file key, five bytes at revision 2.
file_key = hashlib.md5(user + O + struct.pack("<i", P) + file_id).digest()[:5]

# Algorithm 4: U is the padding ciphered under the file key.
U = rc4(file_key, PAD)


def object_key(num: int, gen: int = 0) -> bytes:
    """Algorithm 1: the key a string or stream of this object is ciphered with."""
    return hashlib.md5(file_key + struct.pack("<i", num)[:3] + struct.pack("<i", gen)[:2]).digest()[
        :10
    ]


content = b"BT /F1 18 Tf 40 120 Td (Secret ledger) Tj ET\n"
encrypted_content = rc4(object_key(4), content)

objects = {
    1: b"<< /Type /Catalog /Pages 2 0 R >>",
    2: b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    3: (
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200]"
        b" /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>"
    ),
    4: (
        b"<< /Length %d >>\nstream\n" % len(encrypted_content)
        + encrypted_content
        + b"\nendstream"
    ),
    5: b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    6: (
        b"<< /Filter /Standard /V 1 /R 2 /O <%s> /U <%s> /P %d >>"
        % (O.hex().encode("ascii"), U.hex().encode("ascii"), P)
    ),
}

out = bytearray(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n")
offsets = {}
for num in sorted(objects):
    offsets[num] = len(out)
    out += b"%d 0 obj\n" % num + objects[num] + b"\nendobj\n"

xref_at = len(out)
out += b"xref\n0 %d\n" % (len(objects) + 1)
out += b"0000000000 65535 f \n"
for num in sorted(objects):
    out += b"%010d 00000 n \n" % offsets[num]
out += (
    b"trailer\n<< /Size %d /Root 1 0 R /Encrypt 6 0 R /ID [<%s> <%s>] >>\n"
    % (len(objects) + 1, file_id.hex().encode("ascii"), file_id.hex().encode("ascii"))
)
out += b"startxref\n%d\n%%%%EOF\n" % xref_at

target = Path(__file__).with_name("encrypted.pdf")
target.write_bytes(bytes(out))
print(f"wrote {target} ({len(out)} bytes)")
