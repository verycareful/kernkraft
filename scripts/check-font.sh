#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

# Checks the fonts kernkraft-pdf embeds against other readers.
#
#     scripts/check-font.sh OUT_DIR FONT[:INDEX]...
#
# Each font (a TrueType or OpenType file; INDEX picks a font out of a
# collection, default 0) is embedded as a subset in a one-page PDF that shows
# a line of text with it. Every result must pass `qpdf --check` and
# `mutool info`, be reported by `pdffonts` as an embedded subset with a
# ToUnicode map, give the line back through `pdftotext`, and be drawn by
# poppler as something other than a blank page. The results are then compared
# with poppler's pictures of them by scripts/check-render.sh, so that the
# subsets are drawn by the two renderers alike. A font that lacks a character
# of the line is skipped, and a font that cannot be reduced to a subset (a
# CFF2 font, a font of bitmaps) is reported as unsupported; neither fails the
# run. Exits with 1 when any check fails.

set -euo pipefail

cd "$(dirname "$0")/.."

if [ "$#" -lt 2 ]; then
    echo "usage: scripts/check-font.sh OUT_DIR FONT[:INDEX]..." >&2
    exit 2
fi
out_dir="$1"
shift
mkdir -p "$out_dir"

cargo build -q --release -p kernkraft-pdf --example embed_font
embed=target/release/examples/embed_font

TEXT='Hello, World! The quick brown fox 0123456789'
# The exit status of embed_font for a font with no glyph for a character.
MISSING_GLYPH=3
NOT_REDUCIBLE=4
DPI=36

failed=0
skipped=0
unsupported=0
checked=0
results=()

fail() {
    echo "FAIL $1: $2"
    failed=$((failed + 1))
}

index=0
for spec in "$@"; do
    index=$((index + 1))
    font="${spec%%:*}"
    face=0
    if [ "$spec" != "$font" ]; then
        face="${spec#*:}"
    fi
    name="$(printf '%04d' "$index")-$(basename "$font")-$face"
    copy="$out_dir/$name.pdf"
    status=0
    "$embed" "$font" "$face" "$copy" "$TEXT" 2>"$out_dir/$name.txt" || status=$?
    if [ "$status" -eq "$MISSING_GLYPH" ]; then
        skipped=$((skipped + 1))
        continue
    fi
    if [ "$status" -eq "$NOT_REDUCIBLE" ]; then
        echo "unsupported $name: $(tail -n 1 "$out_dir/$name.txt")"
        unsupported=$((unsupported + 1))
        continue
    fi
    if [ "$status" -ne 0 ]; then
        fail "$name" "not written: $(tail -n 1 "$out_dir/$name.txt")"
        continue
    fi
    checked=$((checked + 1))
    ok=yes
    if ! qpdf --check "$copy" >"$out_dir/$name.qpdf.txt" 2>&1; then
        fail "$name" "qpdf --check: $(grep -m 1 -iE 'error|warning' "$out_dir/$name.qpdf.txt" || true)"
        ok=""
    fi
    if ! mutool info "$copy" >/dev/null 2>&1; then
        fail "$name" "mutool info"
        ok=""
    fi
    # name type encoding emb sub uni: the three flags are the columns 3 from
    # the end of the first row.
    flags="$(pdffonts "$copy" 2>/dev/null | awk 'NR == 3 { print $(NF-4), $(NF-3), $(NF-2) }')"
    if [ "$flags" != "yes yes yes" ]; then
        fail "$name" "pdffonts reports embedded/subset/unicode as: $flags"
        ok=""
    fi
    found="$(pdftotext -f 1 -l 1 "$copy" - 2>/dev/null | tr -s '[:space:]' ' ' | sed 's/ $//')"
    if [ "$found" != "$TEXT" ]; then
        fail "$name" "pdftotext gives '$found'"
        ok=""
    fi
    pdftoppm -r "$DPI" -f 1 -l 1 -cropbox "$copy" "$out_dir/$name.drawn" 2>/dev/null || true
    picture="$(ls "$out_dir/$name".drawn*.ppm 2>/dev/null | head -n 1 || true)"
    if [ -z "$picture" ]; then
        fail "$name" "poppler draws nothing"
        ok=""
    elif ! python3 -I -c '
import sys
data = open(sys.argv[1], "rb").read()
# P6, width, height, maximum: four tokens, then one whitespace byte.
tokens, at = [], 0
while len(tokens) < 4:
    while data[at:at + 1].isspace():
        at += 1
    start = at
    while not data[at:at + 1].isspace():
        at += 1
    tokens.append(data[start:at])
sys.exit(0 if any(b != 255 for b in data[at + 1:]) else 1)
' "$picture"; then
        fail "$name" "poppler draws a blank page"
        ok=""
    fi
    [ -n "$ok" ] && results+=("$copy")
done

if [ "${#results[@]}" -gt 0 ]; then
    if ! scripts/check-render.sh "$out_dir/render" "${results[@]}"; then
        failed=$((failed + 1))
    fi
fi

echo "embedded $checked, skipped $skipped, unsupported $unsupported, failed $failed"
[ "$failed" -eq 0 ]
