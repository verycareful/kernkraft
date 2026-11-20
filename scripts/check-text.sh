#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

# Compares the word positions kernkraft-pdf reads with poppler's.
#
#     scripts/check-text.sh OUT_DIR PDF...
#
# Each PDF that poppler reads without a password is put through
# examples/text_words.rs and through `pdftotext -bbox -cropbox` (the box this
# library displays), and scripts/compare_bbox.py compares the words page by
# page: the left and right edges and the vertical centre of each word, within
# the limits stated there. A file poppler cannot read is skipped. A run that
# compared no file fails, so that a missing input cannot pass as a clean one.
# Exits with 1 when any file fails.

set -uo pipefail

cd "$(dirname "$0")/.."

if [ "$#" -lt 2 ]; then
    echo "usage: scripts/check-text.sh OUT_DIR PDF..." >&2
    exit 2
fi
out_dir="$1"
shift
mkdir -p "$out_dir"

cargo build -q --release -p kernkraft-pdf --example text_words || exit 1
words=target/release/examples/text_words

passed=0
failed=0
skipped=0
index=0

for pdf in "$@"; do
    index=$((index + 1))
    name="$(printf '%04d' "$index")-$(basename "$pdf" .pdf)"
    theirs="$out_dir/$name.theirs.xhtml"
    ours="$out_dir/$name.ours.txt"
    if ! timeout 120 pdftotext -bbox -cropbox "$pdf" "$theirs" 2>/dev/null; then
        echo "skip  $pdf (pdftotext cannot read it)"
        skipped=$((skipped + 1))
        continue
    fi
    if ! timeout 120 "$words" "$pdf" >"$ours" 2>"$out_dir/$name.err"; then
        echo "FAIL  $pdf: $(head -c 200 "$out_dir/$name.err")"
        failed=$((failed + 1))
        continue
    fi
    if python3 -I scripts/compare_bbox.py "$ours" "$theirs" >"$out_dir/$name.report.txt"; then
        passed=$((passed + 1))
    else
        echo "FAIL  $pdf:"
        sed 's/^/    /' "$out_dir/$name.report.txt"
        failed=$((failed + 1))
    fi
done

echo "== text positions: $passed passed, $failed failed, $skipped skipped (not read by poppler)"
[ "$((passed + failed))" -gt 0 ] && [ "$failed" -eq 0 ]
