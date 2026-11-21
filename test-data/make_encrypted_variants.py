#!/usr/bin/env python3
"""Write the encrypted PDF fixtures beside `encrypted.pdf`.

`encrypted.pdf` (by `make_encrypted_pdf.py`) is the RC4 40-bit, revision 2
case with an empty user password. These are the other standard security
handlers a reader meets in practice, each with a *non-empty* user password so
the password path is exercised, not only the empty-password default:

* `encrypted_rc4_128.pdf` — RC4 128-bit, revision 3 (`/V 2 /R 3`).
* `encrypted_aes128.pdf`  — AES-128, revision 4 (`/V 4`, crypt filter `AESV2`).
* `encrypted_aes256.pdf`  — AES-256, revision 6 (`/V 5`, crypt filter `AESV3`).

Every one uses user password `user` and owner password `owner`, carries the
page text `Secret ledger` in a ciphered content stream and the document title
`Ciphered title` in a ciphered `/Info` string. The arithmetic is ISO 32000-2
§7.6 (algorithms 1, 1.A, 2, 2.A, 2.B, 3, 4, 5, 8 and 9), written out by hand so
the fixture is reviewable; `pdftotext -upw user` reading each page back proves
a real reader accepts it. Requires pycryptodome (`Crypto`).
"""

import hashlib
import struct
from pathlib import Path

from Crypto.Cipher import AES

PAD = bytes.fromhex(
    "28BF4E5E4E758A4164004E56FFFA01082E2E00B6D0683E802F0CA9FE6453697A"
)
USER = b"user"
OWNER = b"owner"
P = -3904
FILE_ID = hashlib.md5(b"pdfextractor encrypted variants").digest()
CONTENT = b"BT /F1 18 Tf 40 120 Td (Secret ledger) Tj ET\n"
TITLE = b"Ciphered title"


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


def pad(password: bytes) -> bytes:
    return (password + PAD)[:32]


def aes_cbc_encrypt(key: bytes, data: bytes, iv: bytes) -> bytes:
    """AES-CBC with PKCS#7 padding, the IV written first (ISO 32000 §7.6.3)."""
    n = 16 - len(data) % 16
    data = data + bytes([n]) * n
    return iv + AES.new(key, AES.MODE_CBC, iv).encrypt(data)


# --- revisions 3 and 4: MD5 and RC4 key derivation -------------------------


def legacy_o(length: int) -> bytes:
    """Algorithm 3 at revision 3 or later."""
    key = hashlib.md5(pad(OWNER)).digest()
    for _ in range(50):
        key = hashlib.md5(key[:length]).digest()
    key = key[:length]
    o = pad(USER)
    for i in range(20):
        o = rc4(bytes(b ^ i for b in key), o)
    return o


def legacy_file_key(o: bytes, length: int, encrypt_metadata: bool = True) -> bytes:
    """Algorithm 2 at revision 3 or later."""
    data = pad(USER) + o + struct.pack("<i", P) + FILE_ID
    if not encrypt_metadata:
        data += b"\xff\xff\xff\xff"
    key = hashlib.md5(data).digest()
    for _ in range(50):
        key = hashlib.md5(key[:length]).digest()
    return key[:length]


def legacy_u(file_key: bytes) -> bytes:
    """Algorithm 5: the hash of padding and ID, ciphered 20 times, padded to 32."""
    u = rc4(file_key, hashlib.md5(PAD + FILE_ID).digest())
    for i in range(1, 20):
        u = rc4(bytes(b ^ i for b in file_key), u)
    return u + bytes(16)


def object_key(file_key: bytes, num: int, aes: bool) -> bytes:
    """Algorithm 1: the key one object's strings and streams are ciphered with."""
    data = file_key + struct.pack("<i", num)[:3] + struct.pack("<i", 0)[:2]
    if aes:
        data += b"sAlT"
    return hashlib.md5(data).digest()[: min(len(file_key) + 5, 16)]


# --- revision 6: SHA-2 and AES-256 -----------------------------------------


def hash_2b(password: bytes, salt: bytes, udata: bytes = b"") -> bytes:
    """Algorithm 2.B, the revision 6 password hash."""
    k = hashlib.sha256(password + salt + udata).digest()
    i = 0
    while True:
        k1 = (password + k + udata) * 64
        e = AES.new(k[:16], AES.MODE_CBC, k[16:32]).encrypt(k1)
        m = sum(e[:16]) % 3
        k = [hashlib.sha256, hashlib.sha384, hashlib.sha512][m](e).digest()
        i += 1
        if i >= 64 and e[-1] <= i - 32:
            return k[:32]


def write(name: str, encrypt_dict: bytes, cipher) -> None:
    content = cipher(4, CONTENT)
    title = cipher(7, TITLE)
    objects = {
        1: b"<< /Type /Catalog /Pages 2 0 R >>",
        2: b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        3: (
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200]"
            b" /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>"
        ),
        4: b"<< /Length %d >>\nstream\n" % len(content) + content + b"\nendstream",
        5: b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        6: encrypt_dict,
        7: b"<< /Title <%s> >>" % title.hex().encode("ascii"),
    }
    out = bytearray(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")
    offsets = {}
    for num in sorted(objects):
        offsets[num] = len(out)
        out += b"%d 0 obj\n" % num + objects[num] + b"\nendobj\n"
    xref_at = len(out)
    out += b"xref\n0 %d\n" % (len(objects) + 1)
    out += b"0000000000 65535 f \n"
    for num in sorted(objects):
        out += b"%010d 00000 n \n" % offsets[num]
    hex_id = FILE_ID.hex().encode("ascii")
    out += (
        b"trailer\n<< /Size %d /Root 1 0 R /Info 7 0 R /Encrypt 6 0 R /ID [<%s> <%s>] >>\n"
        % (len(objects) + 1, hex_id, hex_id)
    )
    out += b"startxref\n%d\n%%%%EOF\n" % xref_at
    target = Path(__file__).with_name(name)
    target.write_bytes(bytes(out))
    print(f"wrote {target} ({len(out)} bytes)")


def rc4_128() -> None:
    o = legacy_o(16)
    key = legacy_file_key(o, 16)
    u = legacy_u(key)
    write(
        "encrypted_rc4_128.pdf",
        b"<< /Filter /Standard /V 2 /R 3 /Length 128 /O <%s> /U <%s> /P %d >>"
        % (o.hex().encode(), u.hex().encode(), P),
        lambda num, data: rc4(object_key(key, num, aes=False), data),
    )


def aes_128() -> None:
    o = legacy_o(16)
    key = legacy_file_key(o, 16)
    u = legacy_u(key)
    write(
        "encrypted_aes128.pdf",
        b"<< /Filter /Standard /V 4 /R 4 /Length 128"
        b" /CF << /StdCF << /CFM /AESV2 /AuthEvent /DocOpen /Length 16 >> >>"
        b" /StmF /StdCF /StrF /StdCF /O <%s> /U <%s> /P %d >>"
        % (o.hex().encode(), u.hex().encode(), P),
        lambda num, data: aes_cbc_encrypt(
            object_key(key, num, aes=True), data, hashlib.md5(b"iv%d" % num).digest()
        ),
    )


def aes_256() -> None:
    file_key = hashlib.sha256(b"pdfextractor aes-256 file key").digest()
    # Fixed salts keep the fixture reproducible; a writer would draw them at random.
    u_valid, u_key = b"uvalsalt", b"ukeysalt"
    o_valid, o_key = b"ovalsalt", b"okeysalt"
    u = hash_2b(USER, u_valid) + u_valid + u_key
    ue = AES.new(hash_2b(USER, u_key), AES.MODE_CBC, bytes(16)).encrypt(file_key)
    o = hash_2b(OWNER, o_valid, u) + o_valid + o_key
    oe = AES.new(hash_2b(OWNER, o_key, u), AES.MODE_CBC, bytes(16)).encrypt(file_key)
    # Algorithm 10: /Perms, the permissions ciphered under the file key.
    perms = struct.pack("<i", P) + b"\xff\xff\xff\xff" + b"Tadb" + b"perm"
    perms_enc = AES.new(file_key, AES.MODE_ECB).encrypt(perms)
    write(
        "encrypted_aes256.pdf",
        b"<< /Filter /Standard /V 5 /R 6 /Length 256"
        b" /CF << /StdCF << /CFM /AESV3 /AuthEvent /DocOpen /Length 32 >> >>"
        b" /StmF /StdCF /StrF /StdCF /O <%s> /U <%s> /OE <%s> /UE <%s> /Perms <%s> /P %d >>"
        % (
            o.hex().encode(),
            u.hex().encode(),
            oe.hex().encode(),
            ue.hex().encode(),
            perms_enc.hex().encode(),
            P,
        ),
        lambda num, data: aes_cbc_encrypt(file_key, data, hashlib.md5(b"iv%d" % num).digest()),
    )


rc4_128()
aes_128()
aes_256()
