#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

# Checks text editing against other readers.
#
#     scripts/check-edit-text.sh OUT_DIR [FONT]
#
# examples/edit_text.rs writes a page and edits of its first line (one
# set in the line's own font, one in FONT, embedded, and one saved through an
# edit session as an update appended to the file). Each edit must pass
# `qpdf --check` and `mutool info`, give the new line and the untouched
# footer back through `pdftotext` and no longer give the old line, and
# poppler must draw every row of the page outside the replaced line exactly
# as it drew the page before. Exits with 1 when any check fails.

set -euo pipefail

cd "$(dirname "$0")/.."

if [ "$#" -lt 1 ]; then
    echo "usage: scripts/check-edit-text.sh OUT_DIR [FONT]" >&2
    exit 2
fi
out_dir="$1"
mkdir -p "$out_dir"

cargo build -q --release -p kernkraft-pdf --example edit_text
band="$(target/release/examples/edit_text "$out_dir" ${2+"$2"} | sed -n 's/^band //p')"

failed=0
fail() {
    echo "FAIL $1: $2"
    failed=$((failed + 1))
}

pdftoppm -r 72 "$out_dir/before.pdf" "$out_dir/before"

for after in "$out_dir"/after-*.pdf; do
    name="$(basename "$after" .pdf)"
    qpdf --check "$after" >"$out_dir/$name.qpdf.txt" 2>&1 || fail "$name" "qpdf --check"
    if [ "$name" = after-session ]; then
        size="$(stat -c %s "$out_dir/before.pdf")"
        cmp -s -n "$size" "$out_dir/before.pdf" "$after" || fail "$name" "the file as opened is not a prefix"
    fi
    mutool info "$after" >/dev/null 2>&1 || fail "$name" "mutool info"
    text="$(pdftotext "$after" -)"
    grep -q 'Howdy all' <<<"$text" || fail "$name" "pdftotext lacks the new line"
    grep -q 'Footer text stays' <<<"$text" || fail "$name" "pdftotext lacks the footer"
    if grep -q 'Hello World' <<<"$text"; then
        fail "$name" "pdftotext still gives the old line"
    fi
    pdftoppm -r 72 "$after" "$out_dir/$name"
    python3 - "$out_dir/before-1.ppm" "$out_dir/$name-1.ppm" $band <<'PY' || fail "$name" "pixels outside the line differ"
import sys

def ppm(path):
    data = open(path, "rb").read()
    fields, at = [], 0
    while len(fields) < 4:
        while data[at:at + 1].isspace():
            at += 1
        end = at
        while not data[end:end + 1].isspace():
            end += 1
        fields.append(data[at:end])
        at = end
    return int(fields[1]), int(fields[2]), data[at + 1:]

width, height, a = ppm(sys.argv[1])
w2, h2, b = ppm(sys.argv[2])
assert (width, height) == (w2, h2), "page sizes differ"
top = int(float(sys.argv[3]) * height) - 3
bottom = int(float(sys.argv[4]) * height) + 3
row = width * 3
bad = [y for y in range(height) if not top <= y <= bottom and a[y * row:(y + 1) * row] != b[y * row:(y + 1) * row]]
if bad:
    print("rows differ:", bad[:10])
    sys.exit(1)
PY
done

# With a FONT that has Hebrew, the line edited to a right-to-left paragraph
# with brackets around a right-to-left word (the brackets are drawn mirrored,
# each in an ActualText of the character typed) must be a valid file that
# MuPDF reads back as typed, and poppler must give the brackets as typed too.
bidi="$out_dir/bidi-after.pdf"
if [ -f "$bidi" ]; then
    qpdf --check "$bidi" >"$out_dir/bidi-after.qpdf.txt" 2>&1 || fail bidi-after "qpdf --check"
    mutool info "$bidi" >/dev/null 2>&1 || fail bidi-after "mutool info"
    shalom=$'\u05e9\u05dc\u05d5\u05dd'
    pair=$'(\u05e9\u05dc)'
    mutool draw -F text "$bidi" 2>/dev/null | grep -qF "$shalom $pair" \
        || fail bidi-after "mutool does not give the line as typed"
    pdftotext -raw "$bidi" - | tr -d '\n' | grep -qF '( ' \
        || fail bidi-after "pdftotext lacks the opening bracket"
    pdftotext -raw "$bidi" - | grep -qF ')' || fail bidi-after "pdftotext lacks the closing bracket"
fi

if [ "$failed" -ne 0 ]; then
    echo "$failed checks failed"
    exit 1
fi
echo "edit text: all checks passed"
