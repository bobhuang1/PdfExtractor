#!/usr/bin/env python3
"""Write the encrypted PDF fixtures beside `encrypted.pdf`.

`encrypted.pdf` (by `make_encrypted_pdf.py`) is the RC4 40-bit, revision 2
case with an empty user password. These are the other standard security
handlers a reader meets in practice, each with a *non-empty* user password so
the password path is exercised, not only the empty-password default:

* `encrypted_rc4_128.pdf` — RC4 128-bit, revision 3 (`/V 2 /R 3`).
* `encrypted_aes128.pdf`  — AES-128, revision 4 (`/V 4`, crypt filter `AESV2`).
* `encrypted_aes256.pdf`  — AES-256, revision 6 (`/V 5`, crypt filter `AESV3`).
* `encrypted_objstm.pdf`  — AES-256, revision 6, written the PDF 1.5 way: the
  page tree and `/Info` live in an encrypted object stream, and the file has no
  classic xref table or trailer, only a cross-reference stream (which, per
  ISO 32000-1 7.5.8.2, is never encrypted).
* `encrypted_r5.pdf`      — AES-256, revision 5: the pre-ISO 32000-2 derivation,
  where the password hash is one SHA-256 and algorithm 2.B does not run.
* `encrypted_encryptmetadata.pdf` — AES-128, revision 4, with
  `/EncryptMetadata false`: the `/Type /Metadata` stream stays in the clear
  while the page content and `/Info` string are ciphered.
* `encrypted_cryptfilter.pdf` — revision 4, where `/StmF` names the RC4 filter
  but the page content stream's own `/Filter /Crypt` names `/AltCF`, a second,
  non-default `/CF` entry with `/CFM /None`: that stream is stored in the
  clear while the rest of the file is ciphered.

Every one uses user password `user` and owner password `owner`, carries the
page text `Secret ledger` in a ciphered content stream and the document title
`Ciphered title` in a ciphered `/Info` string (`encrypted_objstm.pdf`'s page
text and title are its own, since both travel through the object stream). The arithmetic is ISO 32000-2
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


def write(
    name: str,
    encrypt_dict: bytes,
    cipher,
    extras: dict | None = None,
    content_extra: bytes = b"",
) -> None:
    content = cipher(4, CONTENT)
    title = cipher(7, TITLE)
    objects = {
        1: b"<< /Type /Catalog /Pages 2 0 R >>",
        2: b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        3: (
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200]"
            b" /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>"
        ),
        4: b"<< /Length %d%s >>\nstream\n"
        % (len(content), content_extra)
        + content
        + b"\nendstream",
        5: b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        6: encrypt_dict,
        7: b"<< /Title <%s> >>" % title.hex().encode("ascii"),
    }
    if extras:
        objects.update(extras)
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


CONTENT_OBJSTM = b"BT /F1 18 Tf 40 120 Td (Object stream page) Tj ET\n"
OBJSTM_TITLE = b"Object stream title"


def object_stream(bodies: list[tuple[int, bytes]]) -> bytes:
    """The decoded `/Type /ObjStm` payload: `objnum offset` pairs, then bodies."""
    offsets = []
    position = 0
    for _, body in bodies:
        offsets.append(position)
        position += len(body) + 1
    header = b" ".join(
        b"%d %d" % (num, offset) for (num, _), offset in zip(bodies, offsets)
    )
    return header + b"\n" + b"".join(body + b" " for _, body in bodies)


def aes_256_object_streams() -> None:
    """Revision 6 over a PDF 1.5 layout: compressed objects and a cross-reference stream.

    The object stream is encrypted whole, like any stream; the specification
    then says the strings inside it are not separately encrypted (ISO 32000-1
    7.5.8.2), so the title comes out of one decryption of that stream. The
    cross-reference stream is never encrypted, and carries `/Root` and
    `/Encrypt` in its dictionary because there is no classic trailer.
    """
    file_key = hashlib.sha256(b"pdfextractor aes-256 file key").digest()
    # Fixed salts keep the fixture reproducible; a writer would draw them at random.
    u_valid, u_key = b"uvalsalt", b"ukeysalt"
    o_valid, o_key = b"ovalsalt", b"okeysalt"
    u = hash_2b(USER, u_valid) + u_valid + u_key
    ue = AES.new(hash_2b(USER, u_key), AES.MODE_CBC, bytes(16)).encrypt(file_key)
    o = hash_2b(OWNER, o_valid, u) + o_valid + o_key
    oe = AES.new(hash_2b(OWNER, o_key, u), AES.MODE_CBC, bytes(16)).encrypt(file_key)
    perms = struct.pack("<i", P) + b"\xff\xff\xff\xff" + b"Tadb" + b"perm"
    perms_enc = AES.new(file_key, AES.MODE_ECB).encrypt(perms)
    encrypt_dict = (
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
        )
    )

    # The catalog stays at the top level; the page tree and /Info are compressed.
    bodies = [
        (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
        (
            3,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200]"
            b" /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
        ),
        (7, b"<< /Title (%s) >>" % OBJSTM_TITLE),
    ]
    decoded = object_stream(bodies)
    first = decoded.index(b"\n") + 1
    compressed = aes_cbc_encrypt(file_key, decoded, hashlib.md5(b"iv8").digest())
    content = aes_cbc_encrypt(file_key, CONTENT_OBJSTM, hashlib.md5(b"iv4").digest())

    out = bytearray(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")
    offsets = {}

    def write_object(num: int, body: bytes) -> None:
        offsets[num] = len(out)
        out.extend(b"%d 0 obj\n" % num + body + b"\nendobj\n")

    write_object(1, b"<< /Type /Catalog /Pages 2 0 R >>")
    write_object(
        4, b"<< /Length %d >>\nstream\n" % len(content) + content + b"\nendstream"
    )
    write_object(5, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
    write_object(6, encrypt_dict)
    write_object(
        8,
        b"<< /Type /ObjStm /N %d /First %d /Length %d >>\nstream\n"
        % (len(bodies), first, len(compressed))
        + compressed
        + b"\nendstream",
    )

    rows = bytearray()

    def entry(kind: int, field2: int, field3: int) -> None:
        rows.extend(bytes([kind]) + field2.to_bytes(4, "big") + field3.to_bytes(2, "big"))

    entry(0, 0, 65535)  # object 0, the free-list head
    entry(1, offsets[1], 0)
    entry(2, 8, 0)  # object 2: first body in the object stream
    entry(2, 8, 1)  # object 3
    entry(1, offsets[4], 0)
    entry(1, offsets[5], 0)
    entry(1, offsets[6], 0)
    entry(2, 8, 2)  # object 7, the /Info dictionary
    entry(1, offsets[8], 0)
    xref_at = len(out)
    entry(1, xref_at, 0)  # object 9, the cross-reference stream itself

    hex_id = FILE_ID.hex().encode("ascii")
    write_object(
        9,
        b"<< /Type /XRef /Size 10 /Root 1 0 R /Encrypt 6 0 R"
        b" /ID [<%s> <%s>] /W [1 4 2] /Length %d >>\nstream\n"
        % (hex_id, hex_id, len(rows))
        + bytes(rows)
        + b"\nendstream",
    )
    out += b"startxref\n%d\n%%%%EOF\n" % xref_at
    target = Path(__file__).with_name("encrypted_objstm.pdf")
    target.write_bytes(bytes(out))
    print(f"wrote {target} ({len(out)} bytes)")


METADATA = (
    b'<?xml version="1.0" encoding="UTF-8"?>\n'
    b'<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF '
    b'xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"/></x:xmpmeta>\n'
)


def aes_256_r5() -> None:
    """AES-256 revision 5: one SHA-256 per salt, without algorithm 2.B."""
    file_key = hashlib.sha256(b"pdfextractor aes-256 file key").digest()

    def hash_r5(password: bytes, salt: bytes, udata: bytes = b"") -> bytes:
        return hashlib.sha256(password + salt + udata).digest()

    u_valid, u_key = b"uvalsalt", b"ukeysalt"
    o_valid, o_key = b"ovalsalt", b"okeysalt"
    u = hash_r5(USER, u_valid) + u_valid + u_key
    ue = AES.new(hash_r5(USER, u_key), AES.MODE_CBC, bytes(16)).encrypt(file_key)
    o = hash_r5(OWNER, o_valid, u) + o_valid + o_key
    oe = AES.new(hash_r5(OWNER, o_key, u), AES.MODE_CBC, bytes(16)).encrypt(file_key)
    perms = struct.pack("<i", P) + b"\xff\xff\xff\xff" + b"Tadb" + b"perm"
    perms_enc = AES.new(file_key, AES.MODE_ECB).encrypt(perms)
    write(
        "encrypted_r5.pdf",
        b"<< /Filter /Standard /V 5 /R 5 /Length 256"
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
        lambda num, data: aes_cbc_encrypt(
            file_key, data, hashlib.md5(b"iv%d" % num).digest()
        ),
    )


def aes_128_encrypt_metadata_false() -> None:
    """Revision 4 with `/EncryptMetadata false`: metadata stays in the clear."""
    o = legacy_o(16)
    key = legacy_file_key(o, 16, encrypt_metadata=False)
    u = legacy_u(key)
    write(
        "encrypted_encryptmetadata.pdf",
        b"<< /Filter /Standard /V 4 /R 4 /Length 128 /EncryptMetadata false"
        b" /CF << /StdCF << /CFM /AESV2 /AuthEvent /DocOpen /Length 16 >> >>"
        b" /StmF /StdCF /StrF /StdCF /O <%s> /U <%s> /P %d >>"
        % (o.hex().encode(), u.hex().encode(), P),
        lambda num, data: aes_cbc_encrypt(
            object_key(key, num, aes=True), data, hashlib.md5(b"iv%d" % num).digest()
        ),
        extras={
            8: b"<< /Type /Metadata /Subtype /XML /Length %d >>\nstream\n"
            % len(METADATA)
            + METADATA
            + b"\nendstream"
        },
    )


def aes_128_crypt_filter() -> None:
    """A revision 4 file whose content stream names its own /Crypt filter.

    `/StmF` is the RC4 filter, so every other stream and string takes /StdCF;
    the page content stream's `/Filter /Crypt` names `/AltCF` instead, a
    non-default `/CF` entry with `/CFM /None`, so its bytes are stored as
    written. Poppler 24.02 only reads an explicit /Crypt filter that leaves
    the bytes alone — its filter case unwraps to the raw stream — so a stream
    ciphered with a different named method cannot be checked with `pdftotext`;
    the method override itself is covered by the unit tests.
    """
    o = legacy_o(16)
    key = legacy_file_key(o, 16)
    u = legacy_u(key)

    def cipher(num: int, data: bytes) -> bytes:
        if num == 4:  # the content stream, which names /AltCF below
            return data
        return rc4(object_key(key, num, aes=False), data)

    write(
        "encrypted_cryptfilter.pdf",
        b"<< /Filter /Standard /V 4 /R 4 /Length 128"
        b" /CF << /StdCF << /CFM /V2 /AuthEvent /DocOpen /Length 16 >>"
        b" /AltCF << /CFM /None >> >>"
        b" /StmF /StdCF /StrF /StdCF /O <%s> /U <%s> /P %d >>"
        % (o.hex().encode(), u.hex().encode(), P),
        cipher,
        content_extra=(
            b" /Filter /Crypt /DecodeParms"
            b" << /Type /CryptFilterDecodeParms /Name /AltCF >>"
        ),
    )


if __name__ == "__main__":
    rc4_128()
    aes_128()
    aes_256()
    aes_256_object_streams()
    aes_256_r5()
    aes_128_encrypt_metadata_false()
    aes_128_crypt_filter()
