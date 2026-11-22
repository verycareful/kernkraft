#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

"""Compares the word positions kernkraft-pdf reads with those of poppler.

    compare_bbox.py OURS THEIRS

OURS is the output of examples/text_words.rs: `page N WIDTH HEIGHT ROTATE`
lines, each followed by `LEFT TOP RIGHT BOTTOM OFF S0X S0Y S3X S3Y E1X E1Y
E2X E2Y WORD` lines whose numbers are fractions of the displayed page except
OFF, which is how many degrees the word's text lies off the nearest axis of
the page. The eight after OFF are the corners of the word's start side (the
first glyph's bottom left and top left) and end side (the last glyph's
bottom right and top right). THEIRS is the output of
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
The edge of a word of ours that its glyphs lean over (text drawn under a
skewing text matrix: the two ends of a glyph's side lie at different places
along the text) is the stretch between the two ends of that side, from the
edge of its box to the edge less the lean; the edge of a word of theirs is
one place. Two edges are within the tolerance when the stretch of one is no
further than that from the other. An upright glyph's side lies at one place
along the text, so for a word of upright glyphs this is the plain
comparison of two edges.
Text turned inside an unturned page is outside this comparison and is
reported as displaced.
A word with no partner of its text goes through four more steps before it
is reported as only ours or only theirs, because the two tools break text
into words by rules of their own (a gap that is a space for one is a kern
for the other, a change of size or baseline inside a run splits a word for
one and not for the other):
  - split: a word of one tool and the words of the other that, read in order
    along the text, spell exactly the same text and together make a box in
    place (by the rule above) are one pair, counted once;
  - unmapped: a word of ours that holds U+FFFD, the character this library
    reads for a glyph that has no Unicode text, is paired by position alone
    with a word of theirs that is in place, so that a font the other tool
    reads by a rule of its own (a glyph name that no list gives text, read as
    its character code) does not fail the page. These pairs are counted on
    their own line of the report, never as words whose text agrees;
  - regrouped: the words of both tools that are left and lie against one
    another (boxes within EDGE_TOLERANCE_PT) make a group; a group in which
    the two tools read the same characters (as a multiset, whatever the
    breaking and the order) and whose boxes, joined on each side, are in
    place, is one pair, counted once. Math is set in runs of different sizes
    and baselines that one tool breaks at each change and the other does not,
    and the words are then not a run of one another, so no spelling of one
    side's words by the other's can be found;
  - oblique: a word of ours more than OBLIQUE_DEGREES off the page's axes is
    outside this comparison (a box that is axis-aligned says too little of
    it), as are the words of theirs whose centre lies in its box, and those
    whose box holds its centre (the other tool joined several turned words
    into one). They are counted on the page's line, never dropped without
    trace.
A word that is still unpaired is reported as only ours or only theirs.

A page passes when no pair is displaced and at least MATCH_LIMIT of its
words, counted on the side that has more, are in place. A split pair counts
as one word on each side, as does a regrouped pair, and an unmapped pair, an oblique word and the
words of theirs set aside with it are not counted. Prints one line for
each page and, for a page that fails, the first words at fault. Exits with 0
when every page passes, and 1 otherwise.
"""

import re
import sys
import unicodedata
import xml.etree.ElementTree as ET
from collections import Counter

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

# Characters XML 1.0 does not allow (Extensible Markup Language 1.0, 2.2,
# production Char). A PDF may hold one (a control character in a word), and
# poppler writes it as it is, so its output is not well formed until they are
# taken out; they are taken out of both sides' words alike.
INVALID_XML = re.compile("[\x00-\x08\x0b\x0c\x0e-\x1f\ufffe\uffff]")

# The character kernkraft-pdf reads for a glyph that has no Unicode text.
UNMAPPED = "\ufffd"

# How far off the page's axes a word's text may lie and still be compared
# as a straight word: 2 degrees, the angle kernkraft-pdf's layout allows
# between the glyphs of one line.
OBLIQUE_DEGREES = 2.0

# The sizes of two pages are the same when they differ by less than this, in
# points: the digits poppler prints.
SIZE_TOLERANCE_PT = 0.01

SHOWN = 5

# The fields of a word line of OURS: four edges, OFF, the eight numbers of the
# start and end sides, and the text.
WORD_FIELDS = 14


def normalise(text):
    folded = unicodedata.normalize("NFKC", INVALID_XML.sub("", text))
    return "".join(c for c in folded if c not in INVISIBLE)


class Word:
    def __init__(self, text, left, top, right, bottom, off=0.0, sides=None, vertical=False):
        """`sides` are the corners, in points, of the start side and the end
        side of the word's glyphs (see read_ours); `vertical` says along
        which axis the text runs. The word's lean at its two edges along the
        text is how far the ends of the side at each edge lie apart along it."""
        self.text = normalise(text)
        self.off = off
        self.left = left
        self.right = right
        self.top = top
        self.bottom = bottom
        self.lean_start = 0.0
        self.lean_end = 0.0
        if sides is not None:
            axis = 1 if vertical else 0
            first = abs(sides[0][axis] - sides[1][axis])
            last = abs(sides[2][axis] - sides[3][axis])
            first_at = (sides[0][axis] + sides[1][axis]) / 2
            last_at = (sides[2][axis] + sides[3][axis]) / 2
            if first_at <= last_at:
                self.lean_start, self.lean_end = first, last
            else:
                self.lean_start, self.lean_end = last, first

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
    (text, left, top, right, bottom, off, sides) in fractions of the page, off
    in degrees and sides four (x, y) pairs, also in fractions."""
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
            fields = line.split(" ", WORD_FIELDS - 1)
            if len(fields) != WORD_FIELDS:
                raise SystemExit(f"{path}:{number}: not a word line: {line!r}")
            try:
                left, top, right, bottom, off = (float(x) for x in fields[:5])
                n = [float(x) for x in fields[5 : WORD_FIELDS - 1]]
            except ValueError:
                raise SystemExit(f"{path}:{number}: not a word line: {line!r}")
            sides = [(n[0], n[1]), (n[2], n[3]), (n[4], n[5]), (n[6], n[7])]
            pages[-1][3].append(
                (fields[WORD_FIELDS - 1], left, top, right, bottom, off, sides)
            )
    return pages


def elements(parent, name):
    """The descendants of `parent` called `name`, whatever their namespace
    (poppler writes the XHTML one)."""
    return [e for e in parent.iter() if e.tag.rsplit("}", 1)[-1] == name]


def read_theirs(path):
    """The pages of THEIRS: each a (width, height, words) in points."""
    pages = []
    with open(path, "rb") as f:
        text = f.read().decode("utf-8")
    root = ET.fromstring(INVALID_XML.sub("", text).encode("utf-8"))
    for page in elements(root, "page"):
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


def edge_spans(word, vertical):
    """The stretches a word's two edges along the text may lie in, (start,
    end), each a (least, greatest): the edge of its box, and the edge less
    (start: plus) its lean. A word whose glyphs do not lean has two places."""
    start, end, _ = along_across(word, vertical)
    return (start, start + word.lean_start), (end - word.lean_end, end)


def span_gap(a, b):
    """How far apart the stretches `a` and `b` are; 0 when they meet."""
    return max(0.0, a[0] - b[1], b[0] - a[1])


def in_place(ours, theirs, vertical):
    o_start, o_end = edge_spans(ours, vertical)
    t_start, t_end = edge_spans(theirs, vertical)
    _, _, o_across = along_across(ours, vertical)
    _, _, t_across = along_across(theirs, vertical)
    thick = max(thickness(ours, vertical), thickness(theirs, vertical))
    return (
        span_gap(o_start, t_start) <= EDGE_TOLERANCE_PT
        and span_gap(o_end, t_end) <= EDGE_TOLERANCE_PT
        and abs(o_across - t_across) <= CENTRE_TOLERANCE * thick
    )


def distance(ours, theirs, vertical):
    o_start, o_end, o_across = along_across(ours, vertical)
    t_start, t_end, t_across = along_across(theirs, vertical)
    return abs(o_start - t_start) + abs(o_end - t_end) + abs(o_across - t_across)


def pair(ours, theirs, vertical, only_in_place):
    """Pairs the words of one page by text: (pairs, only_ours, only_theirs).
    With `only_in_place`, a pair is made only of words in place with each
    other, so that a word is not taken by one of the same text elsewhere."""
    by_text = {}
    for index, word in enumerate(theirs):
        by_text.setdefault(word.text, []).append(index)
    candidates = []
    for i, word in enumerate(ours):
        for j in by_text.get(word.text, ()):
            if only_in_place and not in_place(word, theirs[j], vertical):
                continue
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


def union(words, text, vertical):
    """The word `text` whose box is the smallest one holding `words`. It
    leans as the word that reaches furthest does at each edge."""
    joined = Word(
        text,
        min(w.left for w in words),
        min(w.top for w in words),
        max(w.right for w in words),
        max(w.bottom for w in words),
    )
    starts = [(along_across(w, vertical)[0], -w.lean_start, w) for w in words]
    ends = [(-along_across(w, vertical)[1], -w.lean_end, w) for w in words]
    joined.lean_start = min(starts, key=lambda item: item[:2])[2].lean_start
    joined.lean_end = min(ends, key=lambda item: item[:2])[2].lean_end
    return joined


def across_extent(word, vertical):
    """(least, greatest) of a word's box across its text."""
    if vertical:
        return word.left, word.right
    return word.top, word.bottom


def run_spelling(word, near, parts):
    """The first run of consecutive words of `parts`, taken from the indices
    `near` in order, that spells the text of `word` in two words or more, or
    None."""
    for first in range(len(near)):
        spelt = ""
        for last in range(first, len(near)):
            spelt += parts[near[last]].text
            if not word.text.startswith(spelt):
                break
            if spelt == word.text:
                if last > first:
                    return near[first : last + 1]
                break
    return None


def split_pairs(whole, parts, vertical):
    """Pairs each word of `whole` with a run of words of `parts` that, in
    order along the text, spell its text and make a box in place with it.
    Returns (pairs of (whole word, [part words]), the indices of `whole` and
    of `parts` used). The candidates for a word are the parts whose centre
    lies along its box and whose centre across the text lies in its box: a
    subscript or a raised mark is one of them."""
    pairs, used_whole, used_parts = [], set(), set()
    for i, word in enumerate(whole):
        start, end, _ = along_across(word, vertical)
        low, high = across_extent(word, vertical)
        near = []
        for j, part in enumerate(parts):
            if j in used_parts:
                continue
            p_start, p_end, p_across = along_across(part, vertical)
            if (
                start - EDGE_TOLERANCE_PT <= (p_start + p_end) / 2 <= end + EDGE_TOLERANCE_PT
                and low <= p_across <= high
            ):
                near.append(j)
        near.sort(key=lambda j: along_across(parts[j], vertical)[0])
        for order in ([near, near[::-1]] if vertical else [near]):
            run = run_spelling(word, order, parts)
            if run is None:
                continue
            spelt = "".join(parts[j].text for j in run)
            if in_place(union([parts[j] for j in run], spelt, vertical), word, vertical):
                pairs.append((word, [parts[j] for j in run]))
                used_whole.add(i)
                used_parts.update(run)
                break
    return pairs, used_whole, used_parts


def unmapped_pairs(ours, theirs, vertical):
    """Pairs the words of `ours` that hold U+FFFD with the words of `theirs`
    in place with them, the nearest first: (pairs, used ours, used theirs)."""
    candidates = []
    for i, word in enumerate(ours):
        if UNMAPPED not in word.text:
            continue
        for j, other in enumerate(theirs):
            if in_place(word, other, vertical):
                candidates.append((distance(word, other, vertical), i, j))
    candidates.sort()
    pairs, used_ours, used_theirs = [], set(), set()
    for _, i, j in candidates:
        if i in used_ours or j in used_theirs:
            continue
        used_ours.add(i)
        used_theirs.add(j)
        pairs.append((ours[i], theirs[j]))
    return pairs, used_ours, used_theirs


def touches(a, b):
    """Whether the boxes of `a` and `b` lie against one another, within
    EDGE_TOLERANCE_PT."""
    return (
        a.left - EDGE_TOLERANCE_PT <= b.right
        and b.left - EDGE_TOLERANCE_PT <= a.right
        and a.top - EDGE_TOLERANCE_PT <= b.bottom
        and b.top - EDGE_TOLERANCE_PT <= a.bottom
    )


def characters(words):
    """The characters of `words`, without white space, as a multiset."""
    return Counter(c for w in words for c in w.text if not c.isspace())


def regrouped_pairs(ours, theirs, vertical):
    """Groups the words of `ours` and `theirs` that lie against one another
    and pairs each group in which both tools read the same characters and
    the two joined boxes are in place. Returns (pairs of (words of ours,
    words of theirs), the indices of `ours` and of `theirs` used). A group is
    the words linked, by a word of one tool touching a word of the other, in
    a chain."""
    parent = list(range(len(ours) + len(theirs)))

    def find(node):
        while parent[node] != node:
            parent[node] = parent[parent[node]]
            node = parent[node]
        return node

    for i, mine in enumerate(ours):
        for j, other in enumerate(theirs):
            if touches(mine, other):
                parent[find(i)] = find(len(ours) + j)
    groups = {}
    for node in range(len(parent)):
        groups.setdefault(find(node), []).append(node)
    pairs, used_ours, used_theirs = [], set(), set()
    for members in groups.values():
        mine = [ours[n] for n in members if n < len(ours)]
        other = [theirs[n - len(ours)] for n in members if n >= len(ours)]
        if not mine or not other or characters(mine) != characters(other):
            continue
        if not in_place(union(mine, "", vertical), union(other, "", vertical), vertical):
            continue
        pairs.append((mine, other))
        used_ours.update(n for n in members if n < len(ours))
        used_theirs.update(n - len(ours) for n in members if n >= len(ours))
    return pairs, used_ours, used_theirs


def holds(box, word):
    """Whether the centre of `word` lies in the box of `box`."""
    x = (word.left + word.right) / 2
    y = (word.top + word.bottom) / 2
    return box.left <= x <= box.right and box.top <= y <= box.bottom


def compare_page(number, ours_page, their_page):
    """The report lines for one page, and whether it passes."""
    width, height, rotate, ours = ours_page
    their_width, their_height, theirs = their_page
    vertical = vertical_text(rotate)
    words = [
        Word(
            text,
            left * width,
            top * height,
            right * width,
            bottom * height,
            off,
            [(x * width, y * height) for x, y in sides],
            vertical,
        )
        for text, left, top, right, bottom, off, sides in ours
    ]
    oblique = [w for w in words if w.off > OBLIQUE_DEGREES]
    straight = [w for w in words if w.off <= OBLIQUE_DEGREES]
    kept = [
        t for t in theirs if not any(holds(o, t) or holds(t, o) for o in oblique)
    ]
    set_aside = len(theirs) - len(kept)

    placed, only_ours, only_theirs = pair(straight, kept, vertical, True)

    # Words that one tool breaks and the other does not.
    our_whole, used_w, used_p = split_pairs(only_ours, only_theirs, vertical)
    only_ours_left = [w for i, w in enumerate(only_ours) if i not in used_w]
    only_theirs_left = [w for j, w in enumerate(only_theirs) if j not in used_p]
    their_whole, used_w, used_p = split_pairs(
        only_theirs_left, only_ours_left, vertical
    )
    only_theirs_left = [w for i, w in enumerate(only_theirs_left) if i not in used_w]
    only_ours_left = [w for j, w in enumerate(only_ours_left) if j not in used_p]
    splits = len(our_whole) + len(their_whole)
    extra_ours = sum(len(p) - 1 for _, p in their_whole)
    extra_theirs = sum(len(p) - 1 for _, p in our_whole)

    mapped, used_o, used_t = unmapped_pairs(only_ours_left, only_theirs_left, vertical)
    only_ours_left = [w for i, w in enumerate(only_ours_left) if i not in used_o]
    only_theirs_left = [w for j, w in enumerate(only_theirs_left) if j not in used_t]

    # Words that no spelling of one side by the other joins but that lie in
    # one place and hold the same characters.
    regrouped, used_o, used_t = regrouped_pairs(only_ours_left, only_theirs_left, vertical)
    only_ours_left = [w for i, w in enumerate(only_ours_left) if i not in used_o]
    only_theirs_left = [w for j, w in enumerate(only_theirs_left) if j not in used_t]

    # What is left and has the text of a word on the other side is that word
    # drawn elsewhere.
    displaced, only_ours_left, only_theirs_left = pair(
        only_ours_left, only_theirs_left, vertical, False
    )

    units_ours = (
        len(straight) - extra_ours - len(mapped) - sum(len(o) - 1 for o, _ in regrouped)
    )
    units_theirs = (
        len(kept) - extra_theirs - len(mapped) - sum(len(t) - 1 for _, t in regrouped)
    )
    most = max(units_ours, units_theirs)
    in_place_count = len(placed) + splits + len(regrouped)
    sizes = sorted((width, height)) + sorted((their_width, their_height))
    size_differs = any(
        abs(a - b) > SIZE_TOLERANCE_PT for a, b in zip(sizes[:2], sizes[2:])
    )
    passed = (
        not size_differs
        and not displaced
        and in_place_count >= MATCH_LIMIT * most
    )
    line = (
        f"page {number}: ours {len(words)} theirs {len(theirs)} "
        f"in place {len(placed)} displaced {len(displaced)} "
        f"only ours {len(only_ours_left)} only theirs {len(only_theirs_left)}"
    )
    if splits:
        line += f" split {splits}"
    if mapped:
        line += f" unmapped {len(mapped)}"
    if regrouped:
        line += f" regrouped {len(regrouped)}"
    if oblique:
        line += f" oblique ours {len(oblique)} theirs {set_aside}"
    lines = [line]
    if size_differs:
        lines.append(
            f"  size: ours {width} x {height} (turn {rotate}), "
            f"theirs {their_width} x {their_height}"
        )
    if not passed:
        for o, t in displaced[:SHOWN]:
            lines.append(f"  displaced: ours {o}; theirs {t}")
        for w in only_ours_left[:SHOWN]:
            lines.append(f"  only ours: {w}")
        for w in only_theirs_left[:SHOWN]:
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
