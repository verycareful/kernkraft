#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)
"""Write the encrypted PDF fixtures of testdata/encrypted/.

    python3 -I scripts/make-encrypted-fixtures.py

One small document is written here byte by byte (plain.pdf) and qpdf encrypts
it once for every form of the standard security handler that kernkraft-pdf
reads. The tests of crates/kernkraft-pdf/tests/crypt/ state what the document
holds from the constants below, so a change here is a change there.

qpdf is run with a fixed file identifier and a fixed AES initialisation
vector, so the same qpdf version writes the same bytes again. The files in
the repository were written by qpdf 12.4.2.

qpdf cannot write a file that encrypts only its embedded files (ISO
32000-1:2008, Table 20 EFF, Table 25 AuthEvent EFOpen), so
attachments-only.pdf is written here: the same document, its embedded file
encrypted with RC4 128 (V 4, R 4; 7.6.3.3 Algorithms 2, 3 and 5 for the
keys, 7.6.2 Algorithm 1 for the object's key), everything else plain.
"""

import hashlib
import pathlib
import subprocess
import sys

OUT = pathlib.Path(__file__).resolve().parent.parent / "testdata" / "encrypted"

TITLE = b"Kern encrypted fixture"
# "Grüße" as a UTF-16BE text string, written as a hexadecimal string.
AUTHOR_HEX = b"FEFF004700720075030800DF0065"
CONTENT = b"BT /F1 12 Tf 72 720 Td (Hello, encrypted world) Tj ET\n"
METADATA = (
    b'<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>\n'
    b'<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF '
    b'xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"/></x:xmpmeta>\n'
    b'<?xpacket end="w"?>\n'
)
ATTACHMENT_NAME = b"note.txt"
ATTACHMENT = b"An attached note.\n"

USER = "user"
OWNER = "owner"
# U+00E4, U+00F6 and U+20AC: one byte each in PDFDocEncoding (E4, F6, A0),
# two and three bytes in UTF-8.
ACCENTED = "pässwörd€"

# name -> (user password, owner password, key bits, options inside
# --encrypt, options outside it)
VARIANTS = {
    "rc4-40": (USER, OWNER, 40, [], []),
    "rc4-40-restricted": (USER, OWNER, 40, ["--print=n", "--annotate=n"], []),
    "rc4-128": (USER, OWNER, 128, ["--use-aes=n"], []),
    "rc4-128-restricted": (
        USER,
        OWNER,
        128,
        ["--use-aes=n", "--print=low", "--modify=form", "--extract=n"],
        [],
    ),
    "rc4-128-accented": (ACCENTED, OWNER, 128, ["--use-aes=n"], ["--password-mode=unicode"]),
    # The password's UTF-8 bytes, where revision 3 defines PDFDocEncoding.
    "rc4-128-utf8-bytes": (ACCENTED, OWNER, 128, ["--use-aes=n"], ["--password-mode=bytes"]),
    "rc4-128-object-streams": (USER, OWNER, 128, ["--use-aes=n"], ["--object-streams=generate"]),
    "rc4-128-v4": (USER, OWNER, 128, ["--use-aes=n", "--force-V4"], []),
    "aes-128": (USER, OWNER, 128, ["--use-aes=y"], []),
    "aes-128-cleartext-metadata": (USER, OWNER, 128, ["--use-aes=y", "--cleartext-metadata"], []),
    "aes-128-object-streams": (USER, OWNER, 128, ["--use-aes=y"], ["--object-streams=generate"]),
    "aes-256-r5": (USER, OWNER, 256, ["--force-R5"], []),
    "aes-256": (USER, OWNER, 256, [], []),
    "aes-256-restricted": (
        USER,
        OWNER,
        256,
        ["--print=none", "--modify=none", "--extract=n"],
        [],
    ),
    "aes-256-cleartext-metadata": (USER, OWNER, 256, ["--cleartext-metadata"], []),
    "aes-256-object-streams": (USER, OWNER, 256, [], ["--object-streams=generate"]),
    "aes-256-no-user-password": ("", OWNER, 256, [], []),
    "aes-256-accented": (ACCENTED, OWNER, 256, [], ["--password-mode=unicode"]),
}


# Algorithm 2, step a.
PADDING = bytes.fromhex(
    "28BF4E5E4E758A4164004E56FFFA01082E2E00B6D0683E802F0CA9FE6453697A"
)
# The fixed file identifier of attachments-only.pdf.
FILE_ID = bytes(range(16))
# Table 22: every permission granted, bits 7 and 8 and 13 to 32 set.
PERMISSIONS = -4
KEY_BYTES = 16
REHASHES = 50
RC4_PASSES = 19


def rc4(key: bytes, data: bytes) -> bytes:
    state = list(range(256))
    j = 0
    for i in range(256):
        j = (j + state[i] + key[i % len(key)]) % 256
        state[i], state[j] = state[j], state[i]
    i = j = 0
    out = bytearray()
    for byte in data:
        i = (i + 1) % 256
        j = (j + state[i]) % 256
        state[i], state[j] = state[j], state[i]
        out.append(byte ^ state[(state[i] + state[j]) % 256])
    return bytes(out)


def pad(password: bytes) -> bytes:
    return (password + PADDING)[:32]


def rc4_passes(key: bytes, data: bytes) -> bytes:
    for counter in range(1, RC4_PASSES + 1):
        data = rc4(bytes(byte ^ counter for byte in key), data)
    return data


def owner_entry(owner: bytes, user: bytes) -> bytes:
    """Algorithm 3, revision 3 or greater."""
    digest = hashlib.md5(pad(owner)).digest()
    for _ in range(REHASHES):
        digest = hashlib.md5(digest).digest()
    key = digest[:KEY_BYTES]
    return rc4_passes(key, rc4(key, pad(user)))


def file_key(user: bytes, owner_value: bytes) -> bytes:
    """Algorithm 2, revision 4, metadata encrypted."""
    digest = hashlib.md5(
        pad(user) + owner_value + PERMISSIONS.to_bytes(4, "little", signed=True) + FILE_ID
    ).digest()
    for _ in range(REHASHES):
        digest = hashlib.md5(digest[:KEY_BYTES]).digest()
    return digest[:KEY_BYTES]


def user_entry(key: bytes) -> bytes:
    """Algorithm 5: 16 bytes, then 16 of padding."""
    value = rc4(key, hashlib.md5(PADDING + FILE_ID).digest())
    return rc4_passes(key, value) + bytes(16)


def object_key(key: bytes, number: int, generation: int) -> bytes:
    """Algorithm 1 for RC4."""
    digest = hashlib.md5(
        key + number.to_bytes(3, "little") + generation.to_bytes(2, "little")
    ).digest()
    return digest[: min(len(key) + 5, 16)]


def stream(entries: bytes, data: bytes) -> bytes:
    return b"<<" + entries + b" /Length %d>>\nstream\n" % len(data) + data + b"\nendstream"


def document(attachment: bytes) -> list:
    return [
        b"<</Type /Catalog /Pages 2 0 R /Metadata 6 0 R"
        b" /Names <</EmbeddedFiles <</Names [(" + ATTACHMENT_NAME + b") 8 0 R]>>>>>>",
        b"<</Type /Pages /Kids [3 0 R] /Count 1>>",
        b"<</Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R"
        b" /Resources <</Font <</F1 5 0 R>>>>>>",
        stream(b"", CONTENT),
        b"<</Type /Font /Subtype /Type1 /BaseFont /Helvetica>>",
        stream(b"/Type /Metadata /Subtype /XML", METADATA),
        b"<</Title (" + TITLE + b") /Author <" + AUTHOR_HEX + b">>>",
        b"<</Type /Filespec /F (" + ATTACHMENT_NAME + b") /EF <</F 9 0 R>>>>",
        stream(b"/Type /EmbeddedFile", attachment),
    ]


def write(objects: list, trailer: bytes) -> bytes:
    out = bytearray(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")
    offsets = []
    for number, body in enumerate(objects, start=1):
        offsets.append(len(out))
        out += b"%d 0 obj\n" % number + body + b"\nendobj\n"
    xref = len(out)
    out += b"xref\n0 %d\n" % (len(objects) + 1)
    out += b"0000000000 65535 f \n"
    for offset in offsets:
        out += b"%010d 00000 n \n" % offset
    out += b"trailer\n<</Size %d " % (len(objects) + 1) + trailer + b">>\n"
    out += b"startxref\n%d\n%%%%EOF\n" % xref
    return bytes(out)


def plain() -> bytes:
    return write(document(ATTACHMENT), b"/Root 1 0 R /Info 7 0 R")


# The object number of the embedded file stream in document().
EMBEDDED_FILE_OBJECT = 9


def attachments_only() -> bytes:
    owner_value = owner_entry(OWNER.encode(), USER.encode())
    key = file_key(USER.encode(), owner_value)
    encrypted = rc4(object_key(key, EMBEDDED_FILE_OBJECT, 0), ATTACHMENT)
    objects = document(encrypted)
    objects.append(
        b"<</Filter /Standard /V 4 /R 4 /Length 128 /P %d" % PERMISSIONS
        + b" /O <" + owner_value.hex().encode() + b"> /U <" + user_entry(key).hex().encode()
        + b"> /CF <</StdCF <</CFM /V2 /AuthEvent /EFOpen /Length 16>>>>"
        b" /StmF /Identity /StrF /Identity /EFF /StdCF>>"
    )
    identifier = b"<" + FILE_ID.hex().encode() + b">"
    return write(
        objects,
        b"/Root 1 0 R /Info 7 0 R /Encrypt %d 0 R /ID [" % len(objects)
        + identifier + b" " + identifier + b"]",
    )


def main() -> int:
    OUT.mkdir(parents=True, exist_ok=True)
    source = OUT / "plain.pdf"
    source.write_bytes(plain())
    (OUT / "attachments-only.pdf").write_bytes(attachments_only())
    for name, (user, owner, bits, inside, outside) in VARIANTS.items():
        command = [
            "qpdf",
            "--static-id",
            "--static-aes-iv",
            "--allow-weak-crypto",
            "--compress-streams=n",
            "--object-streams=disable",
            *outside,
            "--encrypt",
            f"--user-password={user}",
            f"--owner-password={owner}",
            f"--bits={bits}",
            *inside,
            "--",
            str(source),
            str(OUT / f"{name}.pdf"),
        ]
        subprocess.run(command, check=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
