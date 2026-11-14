#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)
"""Write the stringprep tables SASLprep uses, from the text of RFC 3454.

    python3 -I scripts/make-saslprep-tables.py rfc3454.txt \
        crates/kernkraft-pdf/src/crypt/saslprep_tables.rs

RFC 4013 (SASLprep) takes these tables of RFC 3454: A.1 (unassigned code
points), B.1 (mapped to nothing), C.1.2 to C.9 (prohibited output) and D.1
and D.2 (bidirectional categories). Each table is read between its
'Start Table' and 'End Table' lines; a line inside is a code point or a
range of them, maybe followed by a semicolon and a comment. The page
headers and footers between are not of that form and are skipped.

The tables are written as sorted, merged ranges of code points.
"""

import re
import sys

TABLES = {
    "A.1": "UNASSIGNED",
    "B.1": "MAPPED_TO_NOTHING",
    "C.1.2": "NON_ASCII_SPACE",
    "C.2.1": "ASCII_CONTROL",
    "C.2.2": "NON_ASCII_CONTROL",
    "C.3": "PRIVATE_USE",
    "C.4": "NON_CHARACTER",
    "C.5": "SURROGATE",
    "C.6": "INAPPROPRIATE_FOR_PLAIN_TEXT",
    "C.7": "INAPPROPRIATE_FOR_CANONICAL",
    "C.8": "CHANGE_DISPLAY_OR_DEPRECATED",
    "C.9": "TAGGING",
    "D.1": "RAND_AL_CAT",
    "D.2": "L_CAT",
}

ENTRY = re.compile(r"^ {3}([0-9A-F]{4,6})(?:-([0-9A-F]{4,6}))?(?:;.*)?$")
RANGES_PER_LINE = 4

HEADER = """// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The stringprep tables of RFC 3454 that SASLprep (RFC 4013, 2.1 to
//! 2.5) uses, as sorted ranges of code points, first and last included.
//! Written by scripts/make-saslprep-tables.py from the text of RFC 3454;
//! not edited by hand.
"""


def read_table(lines, name):
    start = lines.index(f"   ----- Start Table {name} -----")
    end = lines.index(f"   ----- End Table {name} -----")
    ranges = []
    for line in lines[start + 1 : end]:
        match = ENTRY.match(line)
        if not match:
            continue
        first = int(match.group(1), 16)
        last = int(match.group(2), 16) if match.group(2) else first
        if last < first:
            sys.exit(f"table {name}: range {line.strip()} runs backwards")
        ranges.append((first, last))
    if not ranges:
        sys.exit(f"table {name}: no entries found")
    return merge(sorted(ranges))


def merge(ranges):
    merged = [ranges[0]]
    for first, last in ranges[1:]:
        previous_first, previous_last = merged[-1]
        if first <= previous_last + 1:
            merged[-1] = (previous_first, max(previous_last, last))
        else:
            merged.append((first, last))
    return merged


def render(name, table, ranges):
    out = [f"/// Table {name}.", f"pub(super) const {table}: &[(u32, u32)] = &["]
    for index in range(0, len(ranges), RANGES_PER_LINE):
        chunk = ranges[index : index + RANGES_PER_LINE]
        out.append("    " + " ".join(f"(0x{a:04X}, 0x{b:04X})," for a, b in chunk))
    out.append("];")
    return "\n".join(out)


def main():
    if len(sys.argv) != 3:
        sys.exit("usage: make-saslprep-tables.py rfc3454.txt out.rs")
    with open(sys.argv[1], encoding="ascii") as rfc:
        lines = rfc.read().splitlines()
    parts = [HEADER]
    for name, table in TABLES.items():
        parts.append(render(name, table, read_table(lines, name)))
    with open(sys.argv[2], "w", encoding="ascii") as out:
        out.write("\n\n".join(parts) + "\n")


if __name__ == "__main__":
    main()
