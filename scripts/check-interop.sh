#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

# Checks kernkraft-pdf's compacted copies against other readers.
#
#     scripts/check-interop.sh OUT_DIR [PDF...]
#
# Each PDF (by default those of testdata/encrypted/) is opened with the first
# of the passwords below that opens it and written as a compacted copy into
# OUT_DIR. The copy must pass `qpdf --check` and `mutool info`, and must give
# the same text (pdftotext) and the same page images (pdftoppm) as the
# source. Each tool is held to what it makes of the source: a source it
# finds damaged or cannot read is reported and not compared by that tool,
# and a source that is refused here and by qpdf alike is reported as
# refused. Exits with 1 when any check fails, and when no file was checked, so
# that a missing input cannot pass as a clean run.

set -euo pipefail

cd "$(dirname "$0")/.."

if [ "$#" -lt 1 ]; then
    echo "usage: scripts/check-interop.sh OUT_DIR [PDF...]" >&2
    exit 2
fi
out_dir="$1"
shift
if [ "$#" -eq 0 ]; then
    set -- testdata/encrypted/*.pdf
fi
mkdir -p "$out_dir"

# The passwords of the fixtures of testdata/encrypted/.
PASSWORDS=("" "user" "pässwörd€")
# Pages are compared at this resolution: enough to tell any change of
# content, small enough to keep the run short.
RENDER_DPI=36
# The exit code of the compact example for a password that does not open
# the file.
WRONG_PASSWORD=3

cargo build -q --release -p kernkraft-pdf --example compact
compact=target/release/examples/compact

passed=0
failed=0
not_compared=0
refused=0

fail() {
    echo "FAIL $1: $2"
    failed=$((failed + 1))
}

index=0
for pdf in "$@"; do
    # Numbered, since two sources may share a name.
    index=$((index + 1))
    name="$(printf '%04d' "$index")-$(basename "$pdf" .pdf)"
    copy="$out_dir/$name.pdf"
    opened=""
    for password in "${PASSWORDS[@]}"; do
        status=0
        "$compact" "$pdf" "$copy" "$password" 2>"$out_dir/$name.compact.txt" || status=$?
        if [ "$status" -eq 0 ]; then
            opened=yes
            break
        fi
        if [ "$status" -ne "$WRONG_PASSWORD" ]; then
            break
        fi
    done
    if [ -z "$opened" ]; then
        reason="$(tail -n 1 "$out_dir/$name.compact.txt")"
        if qpdf --check "$pdf" >/dev/null 2>&1; then
            fail "$name" "not written: $reason"
        else
            echo "REFUSED $name: $reason (qpdf --check fails on it too)"
            refused=$((refused + 1))
        fi
        continue
    fi
    ok=yes
    if ! qpdf --password="$password" --check "$pdf" >/dev/null 2>&1; then
        echo "NOT COMPARED $name: qpdf --check finds the source damaged"
        not_compared=$((not_compared + 1))
    elif ! qpdf --password="$password" --check "$copy" >"$out_dir/$name.qpdf.txt" 2>&1; then
        fail "$name" "qpdf --check: $(grep -m 1 -iE 'error|warning' "$out_dir/$name.qpdf.txt" || true)"
        ok=""
    fi
    if ! mutool info -p "$password" "$pdf" >/dev/null 2>&1; then
        echo "NOT COMPARED $name: mutool does not open the source"
        not_compared=$((not_compared + 1))
    elif ! mutool info -p "$password" "$copy" >"$out_dir/$name.mutool.txt" 2>&1; then
        fail "$name" "mutool info: $(tail -n 1 "$out_dir/$name.mutool.txt")"
        ok=""
    fi
    # Text and pages, each against the source as the same tool reads it.
    if pdftotext -upw "$password" "$pdf" "$out_dir/$name.source.txt" 2>/dev/null; then
        if ! pdftotext -upw "$password" "$copy" "$out_dir/$name.copy.txt" 2>/dev/null ||
            ! cmp -s "$out_dir/$name.source.txt" "$out_dir/$name.copy.txt"; then
            fail "$name" "pdftotext differs"
            ok=""
        fi
        rm -f "$out_dir/$name".source-*.png "$out_dir/$name".copy-*.png
        pdftoppm -r "$RENDER_DPI" -png -upw "$password" "$pdf" "$out_dir/$name.source"
        pdftoppm -r "$RENDER_DPI" -png -upw "$password" "$copy" "$out_dir/$name.copy"
        for page in "$out_dir/$name".source-*.png; do
            [ -e "$page" ] || continue
            if ! cmp -s "$page" "${page/.source-/.copy-}"; then
                fail "$name" "page $(basename "$page") renders differently"
                ok=""
            fi
        done
    else
        echo "NOT COMPARED $name: poppler does not open the source"
        not_compared=$((not_compared + 1))
    fi
    if [ -n "$ok" ]; then
        passed=$((passed + 1))
    fi
done

echo "== interop: $passed passed, $failed failed, $refused refused as qpdf refuses them, $not_compared not compared by a tool"
[ "$((passed + failed))" -gt 0 ] && [ "$failed" -eq 0 ]
