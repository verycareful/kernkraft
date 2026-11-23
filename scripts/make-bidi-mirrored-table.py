#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)
"""Write the characters that have the Bidi_Mirrored property, and the
characters that stand for their mirror images.

    python3 -I scripts/make-bidi-mirrored-table.py \
        BidiMirroring.txt \
        crates/kernkraft-pdf/src/edit_text/mirrored_table.rs

Unicode Standard Annex 44 (the Unicode Character Database) defines the
Bidi_Mirrored property, field 9 of UnicodeData.txt, and Unicode Standard
Annex 9, rule L4, depicts a character that has it with a mirrored glyph when
its resolved direction is right to left. The characters are read from the
Unicode data of this interpreter, which must be version 16.0.0, the version
the `unicode-bidi` crate's tables are. They are written as sorted, merged
ranges of code points.

BidiMirroring.txt (Bidi_Mirroring_Glyph) gives, for most of them, a
character whose glyph is the mirror image. A pair marked `[BEST FIT]` is
only approximately so and is left out, and so is a pair with a character
that does not have Bidi_Mirrored in Unicode 16.0.0 (the file may be of a
later version). The pairs are written sorted by their first character.
"""

import re
import sys
import unicodedata

UNICODE_VERSION = "16.0.0"
LAST_CODE_POINT = 0x10FFFF
RANGES_PER_LINE = 4


def ranges():
    out = []
    for code in range(LAST_CODE_POINT + 1):
        if unicodedata.mirrored(chr(code)):
            if out and out[-1][1] + 1 == code:
                out[-1][1] = code
            else:
                out.append([code, code])
    return out


PAIR = re.compile(r"^([0-9A-F]{4,6});\s*([0-9A-F]{4,6})\s*#(.*)$")


def pairs(path, mirrored):
    """The exact mirror pairs of BidiMirroring.txt whose two characters are
    both in `mirrored` (a set of code points)."""
    out = []
    with open(path, encoding="utf-8") as f:
        for line in f:
            found = PAIR.match(line.rstrip("\n"))
            if not found:
                continue
            first, second = int(found[1], 16), int(found[2], 16)
            if "[BEST FIT]" in found[3]:
                continue
            if first in mirrored and second in mirrored:
                out.append((first, second))
    return sorted(out)


def main():
    if len(sys.argv) != 3:
        sys.exit("usage: make-bidi-mirrored-table.py BidiMirroring.txt OUTPUT.rs")
    if unicodedata.unidata_version != UNICODE_VERSION:
        sys.exit(
            f"this interpreter has Unicode {unicodedata.unidata_version}, "
            f"not {UNICODE_VERSION}"
        )
    found = ranges()
    members = {code for a, b in found for code in range(a, b + 1)}
    exact = pairs(sys.argv[1], members)
    lines = [
        "// SPDX-License-Identifier: AGPL-3.0-or-later",
        "// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)",
        "",
        "//! The characters with the Bidi_Mirrored property (UAX #44, field 9 of",
        f"//! UnicodeData.txt, Unicode {UNICODE_VERSION}), as sorted, merged ranges of code",
        "//! points, and the exact mirror pairs of BidiMirroring.txt among them. Written",
        "//! by `scripts/make-bidi-mirrored-table.py`; not edited by hand.",
        "",
        "pub(super) const BIDI_MIRRORED: &[(u32, u32)] = &[",
    ]
    for start in range(0, len(found), RANGES_PER_LINE):
        chunk = found[start : start + RANGES_PER_LINE]
        lines.append(
            "    " + " ".join(f"(0x{a:04X}, 0x{b:04X})," for a, b in chunk)
        )
    lines.append("];")
    lines.append("")
    lines.append("/// Each character and the one whose glyph is its mirror image, sorted by the")
    lines.append("/// first.")
    lines.append("pub(super) const MIRROR_PAIRS: &[(u32, u32)] = &[")
    for start in range(0, len(exact), RANGES_PER_LINE):
        chunk = exact[start : start + RANGES_PER_LINE]
        lines.append(
            "    " + " ".join(f"(0x{a:04X}, 0x{b:04X})," for a, b in chunk)
        )
    lines.append("];")
    with open(sys.argv[2], "w", encoding="utf-8") as out:
        out.write("\n".join(lines) + "\n")


main()
