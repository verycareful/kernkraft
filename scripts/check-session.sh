#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

# Checks kernkraft-pdf's incremental update against other readers.
#
#     scripts/check-session.sh OUT_DIR PDF...
#
# Each PDF that qpdf and poppler open without a password is edited in an
# edit session and saved as an incremental update: its title set, its
# first page turned 90 degrees, its first page removed (when it has two or
# more), and the turn and the title appended as two updates in turn. Every
# result must begin with the bytes of the source, pass `qpdf --check` and
# `mutool info`, give the text of every sampled page that the source gave
# (shifted by one for the removal), and show the edit: the title (pdfinfo),
# the rotation, the page count. Exits with 1 when any check fails, and when no
# file was checked, so that a missing input cannot pass as a clean run.

set -euo pipefail

cd "$(dirname "$0")/.."

if [ "$#" -lt 2 ]; then
    echo "usage: scripts/check-session.sh OUT_DIR PDF..." >&2
    exit 2
fi
out_dir="$1"
shift
mkdir -p "$out_dir"

cargo build -q --release -p kernkraft-pdf --example session
session=target/release/examples/session

passed=0
failed=0
skipped=0

fail() {
    echo "FAIL $1: $2"
    failed=$((failed + 1))
}

text() {
    pdftotext -f "$2" -l "$2" "$1" - 2>/dev/null
}

rotation() {
    pdfinfo -f 1 -l 1 "$1" 2>/dev/null | sed -n 's/^Page *1 rot: *//p'
}

title() {
    pdfinfo "$1" 2>/dev/null | sed -n 's/^Title: *//p'
}

sample() {
    printf '%s\n' 1 2 $(($1 / 2)) "$1" | awk '$1 >= 1 && $1 <= '"$1"' && !seen[$1]++'
}

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

# Checks that $1 begins with all the bytes of $2.
kept() {
    if ! cmp -s -n "$(stat -c %s "$2")" "$1" "$2"; then
        fail "$3" "the source's bytes are not the start of the file"
        return 1
    fi
}

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
    if ! count="$(qpdf --show-npages "$pdf" 2>/dev/null)" || [ "$count" -lt 1 ] ||
        ! qpdf --check "$pdf" >/dev/null 2>&1 || ! pdftotext "$pdf" /dev/null 2>/dev/null; then
        skipped=$((skipped + 1))
        continue
    fi
    ok=yes
    run() {
        local case="$1"
        shift
        if ! "$session" "$@" 2>"$out_dir/$name.$case.txt"; then
            # A file whose cross-reference had to be rebuilt is not edited in
            # place; that is the answer, not a failure.
            if grep -q "rebuilt" "$out_dir/$name.$case.txt"; then
                return 2
            fi
            fail "$name.$case" "not written: $(tail -n 1 "$out_dir/$name.$case.txt")"
            ok=""
            return 1
        fi
        kept "$out_dir/$name.$case.pdf" "$pdf" "$name.$case" || ok=""
        sound "$out_dir/$name.$case.pdf" "$name.$case" || ok=""
    }

    copy="$out_dir/$name.title.pdf"
    status=0
    run title title "$pdf" "$copy" "Edited by the session" || status=$?
    if [ "$status" -eq 2 ]; then
        skipped=$((skipped + 1))
        continue
    fi
    if [ "$status" -eq 0 ]; then
        [ "$(title "$copy")" = "Edited by the session" ] || { fail "$name.title" "title not set"; ok=""; }
        for page in $(sample "$count"); do
            same_text "$copy" "$page" "$pdf" "$page" "$name.title" || ok=""
        done
    fi

    copy="$out_dir/$name.rotate.pdf"
    if run rotate rotate "$pdf" "$copy" "1"; then
        want=$((($(rotation "$pdf") + 90) % 360))
        [ "$(rotation "$copy")" -eq "$want" ] || { fail "$name.rotate" "rotation is not $want"; ok=""; }
    fi

    if [ "$count" -ge 2 ]; then
        copy="$out_dir/$name.remove.pdf"
        if run remove remove "$pdf" "$copy" "1"; then
            [ "$(qpdf --show-npages "$copy")" -eq $((count - 1)) ] || { fail "$name.remove" "wrong page count"; ok=""; }
            for page in $(sample $((count - 1))); do
                same_text "$copy" "$page" "$pdf" $((page + 1)) "$name.remove" || ok=""
            done
        fi
    fi

    copy="$out_dir/$name.twice.pdf"
    if run twice twice "$pdf" "$copy" "1" "Appended twice"; then
        want=$((($(rotation "$pdf") + 90) % 360))
        [ "$(rotation "$copy")" -eq "$want" ] || { fail "$name.twice" "rotation is not $want"; ok=""; }
        [ "$(title "$copy")" = "Appended twice" ] || { fail "$name.twice" "title not set"; ok=""; }
    fi

    if [ -n "$ok" ]; then
        passed=$((passed + 1))
    fi
done

echo "== incremental update: $passed passed, $failed failed, $skipped skipped (not opened by qpdf or poppler, or rebuilt)"
[ "$((passed + failed))" -gt 0 ] && [ "$failed" -eq 0 ]
