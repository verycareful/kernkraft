# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)
"""Tests for changelog_section.py. Run: python3 -I scripts/test_changelog_section.py"""

import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import changelog_section  # noqa: E402

SCRIPT = os.path.join(HERE, "changelog_section.py")

THREE = (
    "# Changelog\n"
    "\n"
    "## [0.1.2.0] 2026-03-01\n"
    "\n"
    "Newest body.\n"
    "\n"
    "## [0.1.1.0] 2026-02-01\n"
    "\n"
    "### Added\n"
    "- middle one\n"
    "- middle two\n"
    "\n"
    "## [0.1.0.0] 2026-01-01\n"
    "\n"
    "Oldest body.\n"
)


def run_cli(text: str, label: str) -> subprocess.CompletedProcess:
    with tempfile.TemporaryDirectory(dir=HERE) as d:
        with open(os.path.join(d, "CHANGELOG.md"), "w", encoding="utf-8") as f:
            f.write(text)
        return subprocess.run(
            [sys.executable, "-I", SCRIPT, label, os.path.join(d, "CHANGELOG.md")],
            capture_output=True,
            text=True,
        )


class ChangelogSectionTests(unittest.TestCase):
    def test_extracts_middle_section(self):
        body = changelog_section.extract(THREE, "0.1.1.0")
        self.assertEqual(body, "### Added\n- middle one\n- middle two\n")
        self.assertNotIn("## [", body)

    def test_missing_label_exits_1(self):
        r = run_cli(THREE, "9.9.9.9")
        self.assertEqual(r.returncode, 1)
        self.assertEqual(r.stdout, "")

    def test_empty_section_exits_1(self):
        text = "## [0.1.1.0] x\n\n\n## [0.1.0.0] y\n\nbody\n"
        r = run_cli(text, "0.1.1.0")
        self.assertEqual(r.returncode, 1)
        self.assertEqual(r.stdout, "")

    def test_label_with_regex_metachars_is_literal(self):
        text = "## [0x1x1x0] decoy\n\ndecoy body\n"
        r = run_cli(text, "0.1.1.0")
        self.assertEqual(r.returncode, 1)
        self.assertEqual(r.stdout, "")

    def test_cli_prints_section(self):
        r = run_cli(THREE, "0.1.1.0")
        self.assertEqual(r.returncode, 0)
        self.assertEqual(r.stdout, "### Added\n- middle one\n- middle two\n")


if __name__ == "__main__":
    unittest.main()
