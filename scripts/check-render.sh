#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

# Compares kernkraft-pdf's page renders with pdftoppm's.
#
#     scripts/check-render.sh OUT_DIR [PDF...]
#
# The first page of each PDF is drawn here and by pdftoppm (-cropbox, which is
# the box this renderer draws) at the same
# resolution, and the two pictures are compared by scripts/compare_ppm.py.
# Rasterisers differ at the edges of shapes, so a page passes when its mean
# difference is under MEAN_LIMIT and under FAR_LIMIT of its pixels differ
# strongly. A page this tool cannot draw, or whose size differs, is
# reported; a page that uses content the renderer does not draw yet is
# counted as deferred, not failed, and still compared and printed; a file
# pdftoppm cannot read is skipped. The pictures are kept in
# OUT_DIR. Exits with 1 when any page fails, and when no page passed, so that a
# missing input cannot pass as a clean run.

set -uo pipefail

cd "$(dirname "$0")/.."

if [ "$#" -lt 1 ]; then
    echo "usage: scripts/check-render.sh OUT_DIR [PDF...]" >&2
    exit 2
fi
out_dir="$1"
shift
if [ "$#" -eq 0 ]; then
    set -- testdata/*.pdf
fi
mkdir -p "$out_dir"

DPI=36
MEAN_LIMIT=12
FAR_LIMIT=0.06
FONT_DIRS=(/usr/share/fonts)

cargo build -q --release -p kernkraft-pdf --example render || exit 1
render=target/release/examples/render

passed=0
failed=0
deferred=0
skipped=0

for pdf in "$@"; do
    name="$(basename "$pdf" .pdf)"
    ours="$out_dir/$name.ours.ppm"
    theirs="$out_dir/$name.ref"
    if ! timeout 120 pdftoppm -cropbox -r "$DPI" -f 1 -l 1 -singlefile "$pdf" "$theirs" 2>/dev/null; then
        echo "skip  $pdf (pdftoppm cannot read it)"
        skipped=$((skipped + 1))
        continue
    fi
    if ! timeout 120 "$render" "$pdf" "$ours" 1 "$DPI" "${FONT_DIRS[@]}" \
        >"$out_dir/$name.notes" 2>"$out_dir/$name.err"; then
        echo "FAIL  $pdf: $(head -c 200 "$out_dir/$name.err")"
        failed=$((failed + 1))
        continue
    fi
    result="$(python3 scripts/compare_ppm.py "$ours" "$theirs.ppm")"
    case "$result" in
        size*)
            echo "FAIL  $pdf: $result"
            failed=$((failed + 1))
            ;;
        *)
            mean="$(echo "$result" | awk '{print $2}')"
            far="$(echo "$result" | awk '{print $4}')"
            if awk -v m="$mean" -v f="$far" -v ml="$MEAN_LIMIT" -v fl="$FAR_LIMIT" \
                'BEGIN { exit !(m < ml && f < fl) }'; then
                passed=$((passed + 1))
            elif [ -s "$out_dir/$name.notes" ]; then
                echo "defer $pdf: $result ($(tr '\n' ' ' <"$out_dir/$name.notes"))"
                deferred=$((deferred + 1))
            else
                echo "FAIL  $pdf: $result"
                failed=$((failed + 1))
            fi
            ;;
    esac
done

echo "passed $passed, failed $failed, deferred $deferred, skipped $skipped"
[ "$passed" -gt 0 ] && [ "$failed" -eq 0 ]
