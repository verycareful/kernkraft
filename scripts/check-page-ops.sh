#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

# Checks kernkraft-pdf's page operations against other readers.
#
#     scripts/check-page-ops.sh OUT_DIR PDF...
#
# Each PDF that qpdf and poppler open without a password and that has at
# least two pages is put through extract (the first and the last page),
# remove (the first page), reorder (reversed), rotate (the first page by
# 90) and merge (with itself). Every result must pass `qpdf --check` and
# `mutool info`; the text of each page (pdftotext) must be the text of the
# page it came from; the rotation (pdfinfo) must be the source's plus 90.
# A merge of a file with itself must have twice the pages, in order.
# Exits with 1 when any check fails, and when no file was checked, so that a
# missing input cannot pass as a clean run.

set -euo pipefail

cd "$(dirname "$0")/.."

if [ "$#" -lt 2 ]; then
    echo "usage: scripts/check-page-ops.sh OUT_DIR PDF..." >&2
    exit 2
fi
out_dir="$1"
shift
mkdir -p "$out_dir"

cargo build -q --release -p kernkraft-pdf --example pages
pages=target/release/examples/pages

passed=0
failed=0
skipped=0

fail() {
    echo "FAIL $1: $2"
    failed=$((failed + 1))
}

# The text of page $2 of $1.
text() {
    pdftotext -f "$2" -l "$2" "$1" - 2>/dev/null
}

# The rotation of page 1 of $1.
rotation() {
    pdfinfo -f 1 -l 1 "$1" 2>/dev/null | sed -n 's/^Page *1 rot: *//p'
}

# Some pages of a document of $1 pages: the first two, the middle and the
# last. A manual of a thousand pages is not read a thousand times.
sample() {
    printf '%s\n' 1 2 $(($1 / 2)) "$1" | awk '$1 >= 1 && $1 <= '"$1"' && !seen[$1]++'
}

# Checks that $1 is sound; the name of the case is $2.
sound() {
    if ! qpdf --check "$1" >"$1.qpdf.txt" 2>&1; then
        fail "$2" "qpdf --check: $(grep -m 1 -iE 'error|warning' "$1.qpdf.txt" || true)"
        return 1
    fi
    if ! mutool info "$1" >/dev/null 2>&1; then
        fail "$2" "mutool info"
        return 1
    fi
}

# Checks that page $2 of $1 has the text of page $4 of $3.
same_text() {
    if [ "$(text "$1" "$2")" != "$(text "$3" "$4")" ]; then
        fail "$5" "page $2 has not the text of page $4 of the source"
        return 1
    fi
}

index=0
for pdf in "$@"; do
    index=$((index + 1))
    name="$(printf '%04d' "$index")-$(basename "$pdf" .pdf)"
    if ! count="$(qpdf --show-npages "$pdf" 2>/dev/null)" || [ "$count" -lt 2 ] ||
        ! qpdf --check "$pdf" >/dev/null 2>&1 || ! pdftotext "$pdf" /dev/null 2>/dev/null; then
        skipped=$((skipped + 1))
        continue
    fi
    ok=yes
    run() {
        # run CASE ARGS...: the operation writes $out_dir/$name.CASE.pdf.
        local case="$1"
        shift
        if ! "$pages" "$@" 2>"$out_dir/$name.$case.txt"; then
            fail "$name.$case" "not written: $(tail -n 1 "$out_dir/$name.$case.txt")"
            ok=""
            return 1
        fi
        sound "$out_dir/$name.$case.pdf" "$name.$case" || ok=""
    }

    copy="$out_dir/$name.extract.pdf"
    if run extract extract "$pdf" "$copy" "1,$count"; then
        [ "$(qpdf --show-npages "$copy")" -eq 2 ] || { fail "$name.extract" "not 2 pages"; ok=""; }
        same_text "$copy" 1 "$pdf" 1 "$name.extract" || ok=""
        same_text "$copy" 2 "$pdf" "$count" "$name.extract" || ok=""
    fi

    copy="$out_dir/$name.remove.pdf"
    if run remove remove "$pdf" "$copy" "1"; then
        [ "$(qpdf --show-npages "$copy")" -eq $((count - 1)) ] || { fail "$name.remove" "wrong page count"; ok=""; }
        for page in $(sample $((count - 1))); do
            same_text "$copy" "$page" "$pdf" $((page + 1)) "$name.remove" || ok=""
        done
    fi

    copy="$out_dir/$name.reorder.pdf"
    order="$(seq "$count" -1 1 | paste -sd,)"
    if run reorder reorder "$pdf" "$copy" "$order"; then
        for page in $(sample "$count"); do
            same_text "$copy" "$page" "$pdf" $((count + 1 - page)) "$name.reorder" || ok=""
        done
    fi

    copy="$out_dir/$name.rotate.pdf"
    if run rotate rotate "$pdf" "$copy" "1" 90; then
        want=$((($(rotation "$pdf") + 90) % 360))
        [ "$(rotation "$copy")" -eq "$want" ] || { fail "$name.rotate" "rotation is not $want"; ok=""; }
    fi

    copy="$out_dir/$name.merge.pdf"
    if run merge merge "$copy" "$pdf" "$pdf"; then
        [ "$(qpdf --show-npages "$copy")" -eq $((2 * count)) ] || { fail "$name.merge" "wrong page count"; ok=""; }
        for page in $(sample "$count"); do
            same_text "$copy" "$page" "$pdf" "$page" "$name.merge" || ok=""
            same_text "$copy" $((count + page)) "$pdf" "$page" "$name.merge" || ok=""
        done
    fi

    if [ -n "$ok" ]; then
        passed=$((passed + 1))
    fi
done

echo "== page operations: $passed passed, $failed failed, $skipped skipped (under two pages, or not opened by qpdf or poppler)"
[ "$((passed + failed))" -gt 0 ] && [ "$failed" -eq 0 ]
