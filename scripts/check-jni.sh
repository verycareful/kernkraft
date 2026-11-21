#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

# Checks kernkraft-jni through a JVM, against other readers.
#
#     scripts/check-jni.sh OUT_DIR PDF...
#
# The host build of libkernkraft_jni is loaded by a JVM that declares its
# exports as Kern Android does (scripts/jni-oracle). For each PDF that qpdf
# and poppler open, Oracle opens it, draws the first page 400 pixels wide
# into a direct buffer, turns the page 90 degrees and writes the update to
# append. The picture is compared with pdftoppm's (scripts/compare_ppm.py,
# the limits of check-render.sh); the update appended to the original must
# pass `qpdf --check`, keep the original's bytes as its start, and show the
# page turned. Exits with 1 when any check fails, and when no file was checked,
# so that a missing input cannot pass as a clean run.

set -euo pipefail

cd "$(dirname "$0")/.."

if [ "$#" -lt 2 ]; then
    echo "usage: scripts/check-jni.sh OUT_DIR PDF..." >&2
    exit 2
fi
out_dir="$1"
shift
mkdir -p "$out_dir"

WIDTH_PX=400
MEAN_LIMIT=12
FAR_LIMIT=0.06

cargo build -q --release -p kernkraft-jni
library="$(pwd)/target/release/libkernkraft_jni.so"
rm -rf "$out_dir/classes"
javac -d "$out_dir/classes" scripts/jni-oracle/dev/kern/pdfbridge/*.java

rotation() {
    pdfinfo -f 1 -l 1 "$1" 2>/dev/null | sed -n 's/^Page *1 rot: *//p'
}

passed=0
failed=0
skipped=0

fail() {
    echo "FAIL $1: $2"
    failed=$((failed + 1))
}

index=0
for pdf in "$@"; do
    index=$((index + 1))
    name="$(printf '%04d' "$index")-$(basename "$pdf" .pdf)"
    prefix="$out_dir/$name"
    rm -f "$prefix".*
    if ! qpdf --check "$pdf" >/dev/null 2>&1 || ! pdftotext "$pdf" /dev/null 2>/dev/null; then
        skipped=$((skipped + 1))
        continue
    fi
    if ! java --enable-native-access=ALL-UNNAMED -cp "$out_dir/classes" dev.kern.pdfbridge.Oracle "$library" "$pdf" "$prefix" \
        >"$prefix.oracle.txt" 2>&1; then
        if grep -q 'RebuiltNotEditable' "$prefix.oracle.txt"; then
            skipped=$((skipped + 1))
            continue
        fi
        fail "$name" "oracle: $(tail -n 1 "$prefix.oracle.txt")"
        continue
    fi

    ok=yes
    if timeout 120 pdftoppm -cropbox -scale-to-x "$WIDTH_PX" -scale-to-y -1 -f 1 -l 1 \
        -singlefile "$pdf" "$prefix.theirs" 2>/dev/null; then
        result="$(python3 scripts/compare_ppm.py "$prefix.ppm" "$prefix.theirs.ppm")"
        case "$result" in
        size*) fail "$name" "picture size differs: $result"; ok="" ;;
        *)
            mean="$(sed -n 's/.*mean \([0-9.]*\).*/\1/p' <<<"$result")"
            far="$(sed -n 's/.*far \([0-9.]*\).*/\1/p' <<<"$result")"
            if ! awk -v m="$mean" -v f="$far" -v ml="$MEAN_LIMIT" -v fl="$FAR_LIMIT" \
                'BEGIN { exit !(m < ml && f < fl) }'; then
                fail "$name" "picture differs from pdftoppm's: $result"
                ok=""
            fi
            ;;
        esac
    fi

    edited="$prefix.edited.pdf"
    cat "$pdf" "$prefix.update" >"$edited"
    if ! cmp -s -n "$(stat -c %s "$pdf")" "$edited" "$pdf"; then
        fail "$name" "the source's bytes are not the start of the file"
        ok=""
    fi
    if ! qpdf --check "$edited" >"$prefix.qpdf.txt" 2>&1; then
        fail "$name" "qpdf --check: $(grep -m 1 -iE 'error|warning' "$prefix.qpdf.txt" || true)"
        ok=""
    fi
    want=$((($(rotation "$pdf") + 90) % 360))
    if [ "$(rotation "$edited")" -ne "$want" ]; then
        fail "$name" "the first page is not turned to $want"
        ok=""
    fi
    if [ -n "$ok" ]; then
        passed=$((passed + 1))
    fi
done

echo "passed $passed, failed $failed, skipped $skipped"
[ "$((passed + failed))" -gt 0 ] && [ "$failed" -eq 0 ]
