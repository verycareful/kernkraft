#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

# Core crates never depend on the platform. Fails if any crate other than the
# adapters (kernkraft-tauri, kernkraft-jni) has tauri or jni in its normal or
# build dependency tree.

set -euo pipefail

cd "$(dirname "$0")/.."

ADAPTERS=("kernkraft-tauri" "kernkraft-jni")
status=0

for manifest in crates/*/Cargo.toml; do
    crate="$(basename "$(dirname "$manifest")")"
    for adapter in "${ADAPTERS[@]}"; do
        if [[ "$crate" == "$adapter" ]]; then
            continue 2
        fi
    done
    tree="$(cargo tree -p "$crate" -e normal,build --prefix none --all-features)"
    if grep -Eq '^(tauri|jni) ' <<<"$tree"; then
        echo "error: core crate $crate depends on the platform:" >&2
        grep -E '^(tauri|jni) ' <<<"$tree" | sort -u >&2
        status=1
    else
        echo "ok: $crate"
    fi
done

exit "$status"
