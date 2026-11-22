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
# The last line counts, over the files compared, the words of each of the
# four kinds scripts/compare_bbox.py judges apart from words whose text
# agrees: split (broken differently), regrouped (broken at different places,
# the same characters in one place), unmapped (no Unicode text for a glyph,
# paired by position) and oblique (off the page's axes, not judged).
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
splits=0
regroups=0
unmapped=0
oblique=0

# The sum, over a report's page lines, of the number that follows `$1`.
sum_of() {
    sed -n "s/.* $1 \([0-9][0-9]*\).*/\1/p" "$2" | awk '{ n += $1 } END { print n + 0 }'
}

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
    report="$out_dir/$name.report.txt"
    if python3 -I scripts/compare_bbox.py "$ours" "$theirs" >"$report"; then
        passed=$((passed + 1))
    else
        echo "FAIL  $pdf:"
        sed 's/^/    /' "$report"
        failed=$((failed + 1))
    fi
    splits=$((splits + $(sum_of split "$report")))
    regroups=$((regroups + $(sum_of regrouped "$report")))
    unmapped=$((unmapped + $(sum_of unmapped "$report")))
    oblique=$((oblique + $(sum_of 'oblique ours' "$report")))
done

echo "== text positions: $passed passed, $failed failed, $skipped skipped (not read by poppler)"
echo "== judged apart: $splits split, $regroups regrouped, $unmapped unmapped, $oblique oblique"
[ "$((passed + failed))" -gt 0 ] && [ "$failed" -eq 0 ]
