#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

"""Compares two binary PPM pictures of one page.

    compare_ppm.py A.ppm B.ppm

Prints `size`, the two sizes, when they differ by more than one pixel a side;
otherwise `mean M far F`, the mean absolute difference per channel (0 to 255)
and the fraction of pixels whose channels differ by more than FAR_LEVEL in
any channel. Both pictures are first averaged in blocks of BLOCK by BLOCK
pixels, so the half-pixel shifts and the different anti-aliasing at the edge
of a shape, which every rasteriser has, do not count; a shape drawn in the
wrong place or colour still does. A picture is judged on both numbers: the
mean for the overall tone, the fraction for shapes in the wrong place.
"""

import sys

FAR_LEVEL = 96
BLOCK = 2


def read(path):
    with open(path, "rb") as f:
        data = f.read()
    fields, at = [], 0
    while len(fields) < 4:
        while data[at : at + 1].isspace():
            at += 1
        start = at
        while not data[at : at + 1].isspace():
            at += 1
        fields.append(data[start:at])
        if len(fields) == 4:
            at += 1
    magic, width, height, _ = fields
    if magic != b"P6":
        raise SystemExit(f"{path}: not a binary PPM")
    return int(width), int(height), data[at:]


def blocks(picture, width, height):
    """The picture's top-left width x height pixels, averaged in blocks."""
    stride = picture[0] * 3
    data = picture[2]
    cells = []
    for y in range(0, height - BLOCK + 1, BLOCK):
        for x in range(0, width - BLOCK + 1, BLOCK):
            cell = [0, 0, 0]
            for dy in range(BLOCK):
                at = (y + dy) * stride + x * 3
                for dx in range(BLOCK):
                    for c in range(3):
                        cell[c] += data[at + dx * 3 + c]
            cells.append([v / (BLOCK * BLOCK) for v in cell])
    return cells


def main():
    a, b = read(sys.argv[1]), read(sys.argv[2])
    if abs(a[0] - b[0]) > 1 or abs(a[1] - b[1]) > 1:
        print(f"size {a[0]}x{a[1]} {b[0]}x{b[1]}")
        return
    width, height = min(a[0], b[0]), min(a[1], b[1])
    cells_a, cells_b = blocks(a, width, height), blocks(b, width, height)
    if not cells_a:
        print("mean 0.00 far 0.0000")
        return
    total, far = 0.0, 0
    for ca, cb in zip(cells_a, cells_b):
        diffs = [abs(x - y) for x, y in zip(ca, cb)]
        total += sum(diffs)
        if max(diffs) > FAR_LEVEL:
            far += 1
    print(f"mean {total / (len(cells_a) * 3):.2f} far {far / len(cells_a):.4f}")


main()
