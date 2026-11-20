#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

# Checks the annotations kernkraft-pdf writes against other readers.
#
#     scripts/check-annot.sh OUT_DIR PDF...
#
# Each PDF that qpdf and poppler open without a password gets a highlight, an
# underline, a strikeout, an ink stroke and a note on its first page, and the
# highlight recoloured and the note deleted, saved as an incremental update.
# Every result must begin with the bytes of the source, pass `qpdf --check`
# and `mutool info`, give the text the source gave, and look different from
# the source when poppler draws it (the annotations are drawn). The results
# are then compared with poppler's pictures of them by scripts/check-render.sh,
# so that the appearances this library writes are drawn by the two renderers
# alike. Exits with 1 when any check fails.

set -euo pipefail

cd "$(dirname "$0")/.."

if [ "$#" -lt 2 ]; then
    echo "usage: scripts/check-annot.sh OUT_DIR PDF..." >&2
    exit 2
fi
out_dir="$1"
shift
mkdir -p "$out_dir"

cargo build -q --release -p kernkraft-pdf --example annotate
annotate=target/release/examples/annotate

# Pages are drawn at this resolution to tell whether they differ.
DPI=36

failed=0
skipped=0
checked=0
results=()

fail() {
    echo "FAIL $1: $2"
    failed=$((failed + 1))
}

index=0
for pdf in "$@"; do
    index=$((index + 1))
    name="$(printf '%04d' "$index")-$(basename "$pdf" .pdf)"
    if ! count="$(qpdf --show-npages "$pdf" 2>/dev/null)" || [ "$count" -lt 1 ] ||
        ! qpdf --check "$pdf" >/dev/null 2>&1 || ! pdftotext "$pdf" /dev/null 2>/dev/null; then
        skipped=$((skipped + 1))
        continue
    fi
    copy="$out_dir/$name.pdf"
    if ! "$annotate" "$pdf" "$copy" 2>"$out_dir/$name.txt"; then
        # A file whose cross-reference had to be rebuilt is not edited in
        # place; that is the answer, not a failure.
        if grep -q "rebuilt" "$out_dir/$name.txt"; then
            skipped=$((skipped + 1))
            continue
        fi
        fail "$name" "not written: $(tail -n 1 "$out_dir/$name.txt")"
        continue
    fi
    checked=$((checked + 1))
    ok=yes
    if ! cmp -s -n "$(stat -c %s "$pdf")" "$copy" "$pdf"; then
        fail "$name" "the source's bytes are not the start of the file"
        ok=""
    fi
    if ! qpdf --check "$copy" >"$out_dir/$name.qpdf.txt" 2>&1; then
        fail "$name" "qpdf --check: $(grep -m 1 -iE 'error|warning' "$out_dir/$name.qpdf.txt" || true)"
        ok=""
    fi
    if ! mutool info "$copy" >/dev/null 2>&1; then
        fail "$name" "mutool info"
        ok=""
    fi
    if [ "$(pdftotext -f 1 -l 1 "$copy" - 2>/dev/null)" != "$(pdftotext -f 1 -l 1 "$pdf" - 2>/dev/null)" ]; then
        fail "$name" "the text of page 1 changed"
        ok=""
    fi
    pdftoppm -r "$DPI" -f 1 -l 1 -cropbox "$pdf" "$out_dir/$name.source" 2>/dev/null || true
    pdftoppm -r "$DPI" -f 1 -l 1 -cropbox "$copy" "$out_dir/$name.drawn" 2>/dev/null || true
    source_picture="$(ls "$out_dir/$name".source*.ppm 2>/dev/null | head -n 1 || true)"
    drawn_picture="$(ls "$out_dir/$name".drawn*.ppm 2>/dev/null | head -n 1 || true)"
    if [ -n "$source_picture" ] && [ -n "$drawn_picture" ] && cmp -s "$source_picture" "$drawn_picture"; then
        fail "$name" "poppler draws the page as it was: no annotation shows"
        ok=""
    fi
    [ -n "$ok" ] && results+=("$copy")
done

if [ "${#results[@]}" -gt 0 ]; then
    if ! scripts/check-render.sh "$out_dir/render" "${results[@]}"; then
        failed=$((failed + 1))
    fi
fi

echo "annotated $checked, skipped $skipped, failed $failed"
[ "$failed" -eq 0 ]
