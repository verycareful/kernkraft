#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

"""Compares the word positions kernkraft-pdf reads with those of poppler.

    compare_bbox.py OURS THEIRS

OURS is the output of examples/text_words.rs: `page N WIDTH HEIGHT ROTATE`
lines, each followed by `LEFT TOP RIGHT BOTTOM WORD` lines whose numbers are
fractions of the displayed page. THEIRS is the output of
`pdftotext -bbox -cropbox`, in points on the displayed page (the box this
library displays). The fractions are multiplied by the page size OURS gives,
so both sides are compared in points; the size poppler prints in its page
element is not used for that, because for a page turned 90 or 270 degrees it
is the size before the turn while its words are placed after it. It is
checked as a pair of sides, in either order.

Within a page the words are paired by their text (after Unicode
compatibility normalisation, so that a ligature and its letters are one
text): of all the pairs of one text that two words could make, the nearest
is paired first. Text runs along the page's horizontal axis, or its vertical
axis on a page turned 90 or 270 degrees. A pair is in place when its edges
along the text are within EDGE_TOLERANCE_PT and its centres across the text
are within CENTRE_TOLERANCE of the thicker of the two boxes (the height of a
horizontal word, the width of a vertical one); otherwise it is displaced.
Text turned inside an unturned page is outside this comparison and is
reported as displaced.
A word with no partner of its text is reported as only ours or only theirs,
because the two tools break text into words by rules of their own (a gap
that is a space for one is a kern for the other).

A page passes when no pair is displaced and at least MATCH_LIMIT of its
words, counted on the side that has more, are in place. Prints one line for
each page and, for a page that fails, the first words at fault. Exits with 0
when every page passes, and 1 otherwise.
"""

import sys
import unicodedata
import xml.etree.ElementTree as ET

# The edges along the text follow from the widths of the glyphs and the
# spacing operands, which both tools take from the file, so they agree to the
# rounding of the numbers either prints. A point is a pixel at 72 dpi: a
# word further off than that is drawn in another place.
EDGE_TOLERANCE_PT = 1.0

# The extent of a word's box across the text comes from the font's ascent and
# descent, which this library reads from the font descriptor and poppler from
# the font program, so they differ by the fonts' own disagreement and the
# centres of two boxes on one baseline can be a fraction of an em apart. A quarter of the
# box height admits that and still tells a word on the wrong line, or one
# line off, from one in place.
CENTRE_TOLERANCE = 0.25

# Word breaking differs at kerns, hyphens and the spaces the layout reads
# between glyphs; a page on which a tenth of the words have no partner is
# read differently by the two tools.
MATCH_LIMIT = 0.9

# Characters that print nothing and that one tool may keep and the other drop.
INVISIBLE = {"­", "​", "‌", "‍", "⁠", "﻿"}

# The sizes of two pages are the same when they differ by less than this, in
# points: the digits poppler prints.
SIZE_TOLERANCE_PT = 0.01

SHOWN = 5


def normalise(text):
    folded = unicodedata.normalize("NFKC", text)
    return "".join(c for c in folded if c not in INVISIBLE)


class Word:
    def __init__(self, text, left, top, right, bottom):
        self.text = normalise(text)
        self.left = left
        self.right = right
        self.top = top
        self.bottom = bottom

    @property
    def centre(self):
        return (self.top + self.bottom) / 2

    @property
    def height(self):
        return self.bottom - self.top

    @property
    def width(self):
        return self.right - self.left

    def __str__(self):
        return (
            f"{self.text!r} at {self.left:.1f} {self.top:.1f} "
            f"{self.right:.1f} {self.bottom:.1f}"
        )


def read_ours(path):
    """The pages of OURS: each a (width, height, rotate, words), a word being
    (text, left, top, right, bottom) in fractions of the page."""
    pages = []
    with open(path, encoding="utf-8") as f:
        for number, line in enumerate(f, 1):
            line = line.rstrip("\n")
            if line.startswith("page "):
                fields = line.split(" ")
                try:
                    width, height = float(fields[2]), float(fields[3])
                    rotate = int(fields[4])
                except (IndexError, ValueError):
                    raise SystemExit(f"{path}:{number}: not a page line: {line!r}")
                pages.append((width, height, rotate, []))
                continue
            if not pages:
                raise SystemExit(f"{path}:{number}: a word before the first page")
            fields = line.split(" ", 4)
            if len(fields) != 5:
                raise SystemExit(f"{path}:{number}: not a word line: {line!r}")
            try:
                left, top, right, bottom = (float(x) for x in fields[:4])
            except ValueError:
                raise SystemExit(f"{path}:{number}: not a word line: {line!r}")
            pages[-1][3].append((fields[4], left, top, right, bottom))
    return pages


def elements(parent, name):
    """The descendants of `parent` called `name`, whatever their namespace
    (poppler writes the XHTML one)."""
    return [e for e in parent.iter() if e.tag.rsplit("}", 1)[-1] == name]


def read_theirs(path):
    """The pages of THEIRS: each a (width, height, words) in points."""
    pages = []
    tree = ET.parse(path)
    for page in elements(tree.getroot(), "page"):
        width = float(page.get("width"))
        height = float(page.get("height"))
        words = [
            Word(
                w.text or "",
                float(w.get("xMin")),
                float(w.get("yMin")),
                float(w.get("xMax")),
                float(w.get("yMax")),
            )
            for w in elements(page, "word")
        ]
        pages.append((width, height, words))
    return pages


def vertical_text(rotate):
    """Whether text runs up or down the displayed page."""
    return rotate in (90, 270)


def along_across(word, vertical):
    """(start, end, centre across) of a word, along its text."""
    if vertical:
        return word.top, word.bottom, (word.left + word.right) / 2
    return word.left, word.right, word.centre


def thickness(word, vertical):
    return word.width if vertical else word.height


def in_place(ours, theirs, vertical):
    o_start, o_end, o_across = along_across(ours, vertical)
    t_start, t_end, t_across = along_across(theirs, vertical)
    thick = max(thickness(ours, vertical), thickness(theirs, vertical))
    return (
        abs(o_start - t_start) <= EDGE_TOLERANCE_PT
        and abs(o_end - t_end) <= EDGE_TOLERANCE_PT
        and abs(o_across - t_across) <= CENTRE_TOLERANCE * thick
    )


def distance(ours, theirs, vertical):
    o_start, o_end, o_across = along_across(ours, vertical)
    t_start, t_end, t_across = along_across(theirs, vertical)
    return abs(o_start - t_start) + abs(o_end - t_end) + abs(o_across - t_across)


def pair(ours, theirs, vertical):
    """Pairs the words of one page: (pairs, only_ours, only_theirs)."""
    by_text = {}
    for index, word in enumerate(theirs):
        by_text.setdefault(word.text, []).append(index)
    candidates = []
    for i, word in enumerate(ours):
        for j in by_text.get(word.text, ()):
            candidates.append((distance(word, theirs[j], vertical), i, j))
    candidates.sort()
    used_ours, used_theirs, pairs = set(), set(), []
    for _, i, j in candidates:
        if i in used_ours or j in used_theirs:
            continue
        used_ours.add(i)
        used_theirs.add(j)
        pairs.append((ours[i], theirs[j]))
    only_ours = [w for i, w in enumerate(ours) if i not in used_ours]
    only_theirs = [w for j, w in enumerate(theirs) if j not in used_theirs]
    return pairs, only_ours, only_theirs


def compare_page(number, ours_page, their_page):
    """The report lines for one page, and whether it passes."""
    width, height, rotate, ours = ours_page
    their_width, their_height, theirs = their_page
    vertical = vertical_text(rotate)
    words = [
        Word(text, left * width, top * height, right * width, bottom * height)
        for text, left, top, right, bottom in ours
    ]
    pairs, only_ours, only_theirs = pair(words, theirs, vertical)
    placed = [p for p in pairs if in_place(*p, vertical)]
    displaced = [p for p in pairs if not in_place(*p, vertical)]
    most = max(len(words), len(theirs))
    sizes = sorted((width, height)) + sorted((their_width, their_height))
    size_differs = any(
        abs(a - b) > SIZE_TOLERANCE_PT for a, b in zip(sizes[:2], sizes[2:])
    )
    passed = (
        not size_differs and not displaced and len(placed) >= MATCH_LIMIT * most
    )
    lines = [
        f"page {number}: ours {len(words)} theirs {len(theirs)} "
        f"in place {len(placed)} displaced {len(displaced)} "
        f"only ours {len(only_ours)} only theirs {len(only_theirs)}"
    ]
    if size_differs:
        lines.append(
            f"  size: ours {width} x {height} (turn {rotate}), "
            f"theirs {their_width} x {their_height}"
        )
    if not passed:
        for o, t in displaced[:SHOWN]:
            lines.append(f"  displaced: ours {o}; theirs {t}")
        for w in only_ours[:SHOWN]:
            lines.append(f"  only ours: {w}")
        for w in only_theirs[:SHOWN]:
            lines.append(f"  only theirs: {w}")
    return lines, passed


def compare(ours_pages, theirs_pages):
    """The report lines for a document, and whether every page passes."""
    if len(ours_pages) != len(theirs_pages):
        return (
            [f"page count: ours {len(ours_pages)} theirs {len(theirs_pages)}"],
            False,
        )
    report, ok = [], True
    for number, (ours, theirs) in enumerate(zip(ours_pages, theirs_pages), 1):
        lines, passed = compare_page(number, ours, theirs)
        report.extend(lines)
        ok = ok and passed
    return report, ok


def main(argv):
    if len(argv) != 3:
        print("usage: compare_bbox.py OURS THEIRS", file=sys.stderr)
        return 2
    report, ok = compare(read_ours(argv[1]), read_theirs(argv[2]))
    print("\n".join(report))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
