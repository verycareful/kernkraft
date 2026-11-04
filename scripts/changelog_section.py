#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)
"""Print the body of the `## [<label>]` section of CHANGELOG.md.

Usage: changelog_section.py <label> [changelog-path]

The header is matched literally at the start of a line, so a label's dots are
not wildcards. The body runs to the next `## [` header or the end of the file,
with surrounding blank lines removed. Exit status 1 when the section is missing
or has no body, so a release is never published with empty notes.
"""

import sys


def extract(text: str, label: str) -> str:
    """Return the body of the section for `label`, or "" if absent or empty."""
    header = f"## [{label}]"
    body: list[str] = []
    inside = False
    for line in text.splitlines():
        if inside:
            if line.startswith("## ["):
                break
            body.append(line)
        elif line.startswith(header):
            inside = True
    while body and not body[0].strip():
        body.pop(0)
    while body and not body[-1].strip():
        body.pop()
    return "".join(f"{line}\n" for line in body)


def main(argv: list[str]) -> int:
    if len(argv) not in (2, 3):
        print("usage: changelog_section.py <label> [changelog-path]", file=sys.stderr)
        return 2
    label = argv[1]
    path = argv[2] if len(argv) == 3 else "CHANGELOG.md"
    try:
        with open(path, encoding="utf-8") as f:
            text = f.read()
    except OSError as e:
        print(f"cannot read {path}: {e}", file=sys.stderr)
        return 1
    body = extract(text, label)
    if not body:
        print(f"no non-empty section '## [{label}]' in {path}", file=sys.stderr)
        return 1
    sys.stdout.write(body)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
