# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)
"""Tests for compare_bbox.py. Run: python3 -I scripts/test_compare_bbox.py

Each case writes the two files the script reads, built by hand from the
formats in its header, in a scratch directory inside the repository (ignored
by git, never the system temp directory), and compares them. Every expectation is
worked out from the tolerances in compare_bbox.py, not taken from a run.
"""

import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import compare_bbox  # noqa: E402

REPO = os.path.dirname(HERE)
SCRATCH = os.path.join(REPO, "local", "scratchpad-scripts")

XHTML = (
    '<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.0 Transitional//EN" '
    '"http://www.w3.org/TR/xhtml1/DTD/xhtml1-transitional.dtd">'
    '<html xmlns="http://www.w3.org/1999/xhtml"><head><title></title></head>'
    "<body><doc>{pages}</doc></body></html>"
)


def ours_file(pages):
    """pages: (width, height, rotate, [(text, left, top, right, bottom)]); a
    word may add a sixth item, its degrees off the page's axes (default 0),
    and a seventh, the x of its glyphs' leaning sides as ((start bottom,
    start top), (end bottom, end top)) in fractions (default: upright, the
    box's own edges)."""
    lines = []
    for number, (width, height, rotate, words) in enumerate(pages, 1):
        lines.append(f"page {number} {width} {height} {rotate}")
        for text, left, top, right, bottom, *more in words:
            off = more[0] if more else 0
            (s_bottom, s_top), (e_bottom, e_top) = (
                more[1] if len(more) > 1 else ((left, left), (right, right))
            )
            sides = f"{s_bottom} {bottom} {s_top} {top} {e_bottom} {bottom} {e_top} {top}"
            lines.append(f"{left} {top} {right} {bottom} {off} {sides} {text}")
    return "\n".join(lines) + "\n"


def theirs_file(pages):
    """pages: (width, height, [(text, xmin, ymin, xmax, ymax)]) in points."""
    out = []
    for width, height, words in pages:
        body = "".join(
            f'<word xMin="{x0}" yMin="{y0}" xMax="{x1}" yMax="{y1}">{text}</word>'
            for text, x0, y0, x1, y1 in words
        )
        out.append(f'<page width="{width}" height="{height}">{body}</page>')
    return XHTML.format(pages="".join(out))


class CompareBbox(unittest.TestCase):
    def setUp(self):
        os.makedirs(SCRATCH, exist_ok=True)
        self._tmp = tempfile.TemporaryDirectory(dir=SCRATCH)
        self.addCleanup(self._tmp.cleanup)
        self.root = self._tmp.name

    def run_compare(self, ours, theirs):
        """(report, ok) for `ours` and `theirs`, built by the helpers above."""
        paths = []
        for name, text in (("ours.txt", ours), ("theirs.xhtml", theirs)):
            path = os.path.join(self.root, name)
            with open(path, "w", encoding="utf-8") as f:
                f.write(text)
            paths.append(path)
        return compare_bbox.compare(
            compare_bbox.read_ours(paths[0]), compare_bbox.read_theirs(paths[1])
        )

    # A 200 x 100 point page: a fraction of 0.1 is 20 points across and 10
    # down. "Hello" spans 20..60 and 30..40.

    def test_identical_words_pass(self):
        ours = ours_file([(200, 100, 0, [("Hello", 0.1, 0.3, 0.3, 0.4)])])
        theirs = theirs_file([(200, 100, [("Hello", 20, 30, 60, 40)])])
        report, ok = self.run_compare(ours, theirs)
        self.assertTrue(ok, report)
        self.assertIn("in place 1 displaced 0", report[0])

    def test_edges_within_a_point_pass_and_beyond_fail(self):
        ours = ours_file([(200, 100, 0, [("Hello", 0.1, 0.3, 0.3, 0.4)])])
        near = theirs_file([(200, 100, [("Hello", 20.9, 30, 60.9, 40)])])
        far = theirs_file([(200, 100, [("Hello", 21.1, 30, 60, 40)])])
        self.assertTrue(self.run_compare(ours, near)[1])
        report, ok = self.run_compare(ours, far)
        self.assertFalse(ok)
        self.assertIn("displaced 1", report[0])

    def test_a_different_font_height_is_not_a_displacement(self):
        # The same baseline with a taller box: the centres are 1.5 points
        # apart, under a quarter of the 13 point box.
        ours = ours_file([(200, 100, 0, [("Hello", 0.1, 0.3, 0.3, 0.4)])])
        theirs = theirs_file([(200, 100, [("Hello", 20, 28, 60, 41)])])
        self.assertTrue(self.run_compare(ours, theirs)[1])

    def test_a_word_one_line_off_fails(self):
        ours = ours_file([(200, 100, 0, [("Hello", 0.1, 0.3, 0.3, 0.4)])])
        theirs = theirs_file([(200, 100, [("Hello", 20, 42, 60, 52)])])
        report, ok = self.run_compare(ours, theirs)
        self.assertFalse(ok)
        self.assertIn("displaced 1", report[0])

    def test_the_nearest_of_two_equal_words_is_paired_first(self):
        words = [("the", 0.1, 0.1, 0.2, 0.2), ("the", 0.6, 0.6, 0.7, 0.7)]
        theirs_words = [("the", 120, 60, 140, 70), ("the", 20, 10, 40, 20)]
        report, ok = self.run_compare(
            ours_file([(200, 100, 0, words)]),
            theirs_file([(200, 100, theirs_words)]),
        )
        self.assertTrue(ok, report)

    def test_a_ligature_and_its_letters_are_one_text(self):
        ours = ours_file([(200, 100, 0, [("ﬁsh", 0.1, 0.3, 0.3, 0.4)])])
        theirs = theirs_file([(200, 100, [("fish", 20, 30, 60, 40)])])
        self.assertTrue(self.run_compare(ours, theirs)[1])

    @staticmethod
    def numbered(count, missing):
        """`count` words w0.. in a row on both sides, theirs without the last
        `missing`."""
        ours = [
            (f"w{i}", 0.01 * i, 0.3, 0.01 * i + 0.005, 0.4) for i in range(count)
        ]
        theirs = [
            (f"w{i}", 2 * i, 30, 2 * i + 1, 40) for i in range(count - missing)
        ]
        return ours_file([(200, 100, 0, ours)]), theirs_file([(200, 100, theirs)])

    def test_more_than_a_tenth_unmatched_fails_and_is_reported(self):
        # 20 words, 3 with no partner: 17 are in place, under 18.
        report, ok = self.run_compare(*self.numbered(20, 3))
        self.assertFalse(ok)
        self.assertIn("in place 17 displaced 0 only ours 3 only theirs 0", report[0])
        self.assertIn("only ours: 'w19'", "\n".join(report))

    def test_a_tenth_unmatched_passes_and_is_still_reported(self):
        # 20 words, 2 with no partner: 18 are in place, a tenth short.
        report, ok = self.run_compare(*self.numbered(20, 2))
        self.assertTrue(ok, report)
        self.assertIn("only ours 2", report[0])

    def test_words_on_one_side_only_fail(self):
        ours = ours_file([(200, 100, 0, [])])
        theirs = theirs_file([(200, 100, [("Hello", 20, 30, 60, 40)])])
        report, ok = self.run_compare(ours, theirs)
        self.assertFalse(ok)
        self.assertIn("only theirs 1", report[0])

    def test_a_page_with_no_words_on_either_side_passes(self):
        report, ok = self.run_compare(
            ours_file([(200, 100, 0, [])]), theirs_file([(200, 100, [])])
        )
        self.assertTrue(ok, report)

    def test_a_page_count_that_differs_fails(self):
        ours = ours_file([(200, 100, 0, []), (200, 100, 0, [])])
        report, ok = self.run_compare(ours, theirs_file([(200, 100, [])]))
        self.assertFalse(ok)
        self.assertIn("page count", report[0])

    def test_a_page_size_that_differs_fails(self):
        ours = ours_file([(200, 100, 0, [("Hello", 0.1, 0.3, 0.3, 0.4)])])
        theirs = theirs_file([(210, 100, [("Hello", 20, 30, 60, 40)])])
        report, ok = self.run_compare(ours, theirs)
        self.assertFalse(ok)
        self.assertIn("size:", "\n".join(report))

    # A page whose crop box is smaller than its media box. The fractions of
    # ours are of the crop box, and `pdftotext -bbox -cropbox` measures from
    # its corner; without -cropbox poppler measures from the media box's, so
    # every word comes out shifted by the crop box's offset.

    def test_a_crop_box_frame_is_compared_in_the_crop_box(self):
        ours = ours_file([(200, 100, 0, [("Hello", 0.1, 0.3, 0.3, 0.4)])])
        in_crop = theirs_file([(200, 100, [("Hello", 20, 30, 60, 40)])])
        in_media = theirs_file([(200, 100, [("Hello", 50, 45, 90, 55)])])
        self.assertTrue(self.run_compare(ours, in_crop)[1])
        self.assertFalse(self.run_compare(ours, in_media)[1])

    # A page turned 90 degrees: ours is the displayed 100 x 200, poppler
    # prints the size before the turn (200 x 100) with words placed after it.
    # The text runs down the page, so its edges are the top and bottom and
    # its thickness is the width.

    def test_a_turned_page_is_compared_down_the_text(self):
        ours = ours_file([(100, 200, 90, [("Hello", 0.1, 0.1, 0.2, 0.3)])])
        # Ours covers x 10..20 and y 20..60. A box 3 points wider across
        # the text, on the same centre line, with the same ends.
        theirs = theirs_file([(200, 100, [("Hello", 8.5, 20, 21.5, 60)])])
        report, ok = self.run_compare(ours, theirs)
        self.assertTrue(ok, report)

    def test_a_turned_page_does_not_judge_the_ends_across_the_text(self):
        # The same words read as horizontal text are 1.5 points off at the
        # left edge: the axis follows the page's turn.
        ours = ours_file([(100, 200, 0, [("Hello", 0.1, 0.1, 0.2, 0.3)])])
        theirs = theirs_file([(100, 200, [("Hello", 8.5, 20, 21.5, 60)])])
        self.assertFalse(self.run_compare(ours, theirs)[1])

    def test_a_turned_page_with_a_word_down_the_text_fails(self):
        ours = ours_file([(100, 200, 90, [("Hello", 0.1, 0.1, 0.2, 0.3)])])
        theirs = theirs_file([(200, 100, [("Hello", 10, 23, 20, 63)])])
        self.assertFalse(self.run_compare(ours, theirs)[1])

    def test_a_malformed_ours_line_is_refused(self):
        path = os.path.join(self.root, "ours.txt")
        with open(path, "w", encoding="utf-8") as f:
            f.write("page 1 200 100 0\nnot a word\n")
        with self.assertRaises(SystemExit):
            compare_bbox.read_ours(path)

    def test_a_word_before_the_first_page_is_refused(self):
        path = os.path.join(self.root, "ours.txt")
        with open(path, "w", encoding="utf-8") as f:
            f.write("0.1 0.1 0.2 0.2 Hello\n")
        with self.assertRaises(SystemExit):
            compare_bbox.read_ours(path)

    # A character XML 1.0 does not allow is taken out of both sides.

    def test_a_control_character_in_theirs_is_taken_out(self):
        ours = ours_file([(200, 100, 0, [("Hello", 0.1, 0.3, 0.3, 0.4)])])
        theirs = theirs_file([(200, 100, [("He\x1cllo", 20, 30, 60, 40)])])
        report, ok = self.run_compare(ours, theirs)
        self.assertTrue(ok, report)
        self.assertIn("in place 1 displaced 0 only ours 0 only theirs 0", report[0])

    def test_a_control_character_in_ours_is_taken_out(self):
        ours = ours_file([(200, 100, 0, [("He\x1cllo", 0.1, 0.3, 0.3, 0.4)])])
        theirs = theirs_file([(200, 100, [("Hello", 20, 30, 60, 40)])])
        report, ok = self.run_compare(ours, theirs)
        self.assertTrue(ok, report)

    # A word one tool breaks and the other does not: the pieces, in order
    # along the text, must spell it exactly and make a box in place with it.
    # "Hello" spans 20..60.

    def test_a_word_theirs_breaks_is_one_split_pair(self):
        ours = ours_file([(200, 100, 0, [("Hello", 0.1, 0.3, 0.3, 0.4)])])
        theirs = theirs_file(
            [(200, 100, [("He", 20, 30, 32, 40), ("llo", 32.4, 30, 60, 40)])]
        )
        report, ok = self.run_compare(ours, theirs)
        self.assertTrue(ok, report)
        self.assertIn("in place 0 displaced 0 only ours 0 only theirs 0 split 1", report[0])

    def test_a_word_ours_breaks_is_one_split_pair(self):
        ours = ours_file(
            [(200, 100, 0, [("He", 0.1, 0.3, 0.16, 0.4), ("llo", 0.162, 0.3, 0.3, 0.4)])]
        )
        theirs = theirs_file([(200, 100, [("Hello", 20, 30, 60, 40)])])
        report, ok = self.run_compare(ours, theirs)
        self.assertTrue(ok, report)
        self.assertIn("only ours 0 only theirs 0 split 1", report[0])

    def test_pieces_that_spell_another_text_are_not_a_split_pair(self):
        ours = ours_file([(200, 100, 0, [("Hello", 0.1, 0.3, 0.3, 0.4)])])
        theirs = theirs_file(
            [(200, 100, [("He", 20, 30, 32, 40), ("lo", 32.4, 30, 60, 40)])]
        )
        report, ok = self.run_compare(ours, theirs)
        self.assertFalse(ok, report)
        self.assertIn("only ours 1 only theirs 2", report[0])

    def test_pieces_whose_box_is_not_in_place_are_not_a_split_pair(self):
        # The pieces' union ends at 70, ten points past the word's 60.
        ours = ours_file([(200, 100, 0, [("Hello", 0.1, 0.3, 0.3, 0.4)])])
        theirs = theirs_file(
            [(200, 100, [("He", 20, 30, 32, 40), ("llo", 40, 30, 70, 40)])]
        )
        report, ok = self.run_compare(ours, theirs)
        self.assertFalse(ok, report)
        self.assertNotIn("split", report[0])

    def test_a_stray_word_after_the_pieces_does_not_stop_the_pair(self):
        # "zz" lies within a point of the word's end but is not part of the
        # spelling: the run He + llo is found and "zz" is left over.
        ours = ours_file([(200, 100, 0, [("Hello", 0.1, 0.3, 0.3, 0.4)])])
        theirs = theirs_file(
            [
                (
                    200,
                    100,
                    [
                        ("He", 20, 30, 32, 40),
                        ("llo", 32.4, 30, 60, 40),
                        ("zz", 59, 30, 60.4, 40),
                    ],
                )
            ]
        )
        report, ok = self.run_compare(ours, theirs)
        self.assertFalse(ok, report)
        self.assertIn("only ours 0 only theirs 1 split 1", report[0])

    def test_a_lowered_piece_inside_the_word_box_is_a_part(self):
        # "x" and a smaller, lowered "1" make "x1": the "1" has its centre,
        # 36.5, inside the word's 30..40 across the text.
        ours = ours_file([(200, 100, 0, [("x1", 0.1, 0.3, 0.2, 0.4)])])
        theirs = theirs_file(
            [(200, 100, [("x", 20, 30, 30, 40), ("1", 30, 33, 40, 40)])]
        )
        report, ok = self.run_compare(ours, theirs)
        self.assertTrue(ok, report)
        self.assertIn("split 1", report[0])

    def test_pieces_down_a_turned_page_may_read_against_the_axis(self):
        # Text climbs a page turned 270: the first letters are lowest, so
        # the pieces spell the word from the greater end downwards.
        ours = ours_file([(100, 200, 270, [("Hello", 0.2, 0.2, 0.3, 0.5)])])
        theirs = theirs_file(
            [(100, 200, [("He", 20, 70, 30, 100), ("llo", 20, 40, 30, 69.6)])]
        )
        report, ok = self.run_compare(ours, theirs)
        self.assertTrue(ok, report)
        self.assertIn("split 1", report[0])

    # A word of ours that holds U+FFFD has no text to compare: it is paired
    # with a word of theirs by position alone and counted on its own.

    def test_an_unmapped_word_is_paired_by_position_and_counted(self):
        ours = ours_file([(200, 100, 0, [("\ufffd\ufffd", 0.1, 0.3, 0.3, 0.4)])])
        theirs = theirs_file([(200, 100, [("JJ", 20, 30, 60, 40)])])
        report, ok = self.run_compare(ours, theirs)
        self.assertTrue(ok, report)
        self.assertIn("only ours 0 only theirs 0 unmapped 1", report[0])

    def test_an_unmapped_word_out_of_place_is_not_paired(self):
        ours = ours_file([(200, 100, 0, [("\ufffd\ufffd", 0.1, 0.3, 0.3, 0.4)])])
        theirs = theirs_file([(200, 100, [("JJ", 120, 30, 160, 40)])])
        report, ok = self.run_compare(ours, theirs)
        self.assertFalse(ok, report)
        self.assertIn("only ours 1 only theirs 1", report[0])
        self.assertNotIn("unmapped", report[0])

    def test_unmapped_words_do_not_hide_a_displaced_word(self):
        ours = ours_file(
            [(200, 100, 0, [("\ufffd", 0.1, 0.3, 0.2, 0.4), ("Hello", 0.1, 0.6, 0.3, 0.7)])]
        )
        theirs = theirs_file(
            [(200, 100, [("J", 20, 30, 40, 40), ("Hello", 20, 85, 60, 95)])]
        )
        report, ok = self.run_compare(ours, theirs)
        self.assertFalse(ok, report)
        self.assertIn("displaced 1", report[0])
        self.assertIn("unmapped 1", report[0])

    # A word of ours more than 2 degrees off the page's axes, and the words
    # of theirs whose centre lies in its box, are set aside and counted.

    def test_an_oblique_word_and_the_pieces_in_its_box_are_set_aside(self):
        ours = ours_file(
            [
                (
                    200,
                    100,
                    0,
                    [
                        ("height", 0.1, 0.3, 0.2, 0.6, 30.0),
                        ("Hello", 0.5, 0.3, 0.7, 0.4),
                    ],
                )
            ]
        )
        theirs = theirs_file(
            [
                (
                    200,
                    100,
                    [
                        ("hei", 22, 30, 38, 42),
                        ("gh", 24, 42, 38, 52),
                        ("t", 28, 52, 38, 58),
                        ("Hello", 100, 30, 140, 40),
                    ],
                )
            ]
        )
        report, ok = self.run_compare(ours, theirs)
        self.assertTrue(ok, report)
        self.assertIn("in place 1 displaced 0 only ours 0 only theirs 0", report[0])
        self.assertIn("oblique ours 1 theirs 3", report[0])

    def test_a_word_of_theirs_outside_an_oblique_box_is_not_set_aside(self):
        ours = ours_file(
            [
                (
                    200,
                    100,
                    0,
                    [
                        ("height", 0.1, 0.3, 0.2, 0.6, 30.0),
                        ("Hello", 0.5, 0.3, 0.7, 0.4),
                    ],
                )
            ]
        )
        theirs = theirs_file(
            [(200, 100, [("hei", 22, 30, 38, 42), ("Hello", 100, 30, 140, 40), ("stray", 160, 70, 190, 80)])]
        )
        report, ok = self.run_compare(ours, theirs)
        self.assertFalse(ok, report)
        self.assertIn("only theirs 1", report[0])
        self.assertIn("oblique ours 1 theirs 1", report[0])

    def test_a_word_of_theirs_that_holds_an_oblique_word_is_set_aside(self):
        # The other tool joined two turned digits into one wide word whose
        # centre is in neither of ours.
        ours = ours_file(
            [
                (
                    200,
                    100,
                    0,
                    [
                        ("1", 0.1, 0.3, 0.12, 0.4, 3.8),
                        ("2", 0.3, 0.3, 0.32, 0.4, 3.8),
                        ("Hello", 0.5, 0.6, 0.7, 0.7),
                    ],
                )
            ]
        )
        theirs = theirs_file(
            [(200, 100, [("12", 20, 30, 64, 40), ("Hello", 100, 60, 140, 70)])]
        )
        report, ok = self.run_compare(ours, theirs)
        self.assertTrue(ok, report)
        self.assertIn("in place 1 displaced 0 only ours 0 only theirs 0", report[0])
        self.assertIn("oblique ours 2 theirs 1", report[0])

    def test_a_word_within_two_degrees_is_compared_as_straight(self):
        ours = ours_file([(200, 100, 0, [("Hello", 0.1, 0.3, 0.3, 0.4, 2.0)])])
        theirs = theirs_file([(200, 100, [("Hello", 20, 30, 60, 40)])])
        report, ok = self.run_compare(ours, theirs)
        self.assertTrue(ok, report)
        self.assertNotIn("oblique", report[0])

    def test_a_word_past_two_degrees_is_oblique(self):
        ours = ours_file([(200, 100, 0, [("Hello", 0.1, 0.3, 0.3, 0.4, 2.1)])])
        theirs = theirs_file([(200, 100, [])])
        report, ok = self.run_compare(ours, theirs)
        self.assertTrue(ok, report)
        self.assertIn("oblique ours 1 theirs 0", report[0])

    # A word of ours whose glyphs lean right by 1.3 points over their 10
    # point height: 0.0065 of the 200 point page. Its start side runs from
    # x 20 (bottom) to 21.3 (top), its end side from 58.7 to 60, so its box
    # is 20..60 and its edges may lie anywhere in 20..21.3 and 58.7..60.
    LEAN = ((0.1, 0.1065), (0.2935, 0.3))

    def leaning(self, start, end):
        ours = ours_file(
            [(200, 100, 0, [("Hello", 0.1, 0.3, 0.3, 0.4, 0, self.LEAN)])]
        )
        theirs = theirs_file([(200, 100, [("Hello", start, 30, end, 40)])])
        return self.run_compare(ours, theirs)

    def test_a_leaning_word_may_have_its_edges_on_the_lean(self):
        report, ok = self.leaning(21.3, 58.7)
        self.assertTrue(ok, report)
        self.assertIn("in place 1 displaced 0", report[0])

    def test_the_tolerance_applies_beyond_the_lean(self):
        # 22.2 is 0.9 from the stretch 20..21.3, and 57.8 is 0.9 from
        # 58.7..60.
        self.assertTrue(self.leaning(22.2, 57.8)[1])
        # 22.4 and 57.6 are 1.1 from them.
        for start, end in ((22.4, 58.7), (21.3, 57.6)):
            report, ok = self.leaning(start, end)
            self.assertFalse(ok, (start, end))
            self.assertIn("displaced 1", report[0])

    def test_the_same_edges_displace_a_word_that_does_not_lean(self):
        ours = ours_file([(200, 100, 0, [("Hello", 0.1, 0.3, 0.3, 0.4)])])
        theirs = theirs_file([(200, 100, [("Hello", 21.3, 30, 58.7, 40)])])
        report, ok = self.run_compare(ours, theirs)
        self.assertFalse(ok)
        self.assertIn("displaced 1", report[0])

    def test_text_that_runs_against_the_axis_leans_at_the_edge_it_is_on(self):
        # Turned a half: the start side is on the right, and its lean is
        # at the right edge (58.7..60 as above), the end side's on the left.
        lean = ((0.2935, 0.3), (0.1, 0.1065))
        ours = ours_file(
            [(200, 100, 0, [("Hello", 0.1, 0.3, 0.3, 0.4, 0, lean)])]
        )
        theirs = theirs_file([(200, 100, [("Hello", 21.3, 30, 58.7, 40)])])
        report, ok = self.run_compare(ours, theirs)
        self.assertTrue(ok, report)

    def test_a_leaning_word_is_compared_along_a_vertical_text(self):
        # A page turned 90: text runs down. The word's box is 20..60 down
        # the 100 by 200 page's height (0.1..0.3 of 200) and its start side
        # leans 1.3 points along y (bottom y 20, top y 21.3), its x the
        # same for both ends.
        lines = [
            "page 1 100 200 90",
            "0.1 0.1 0.2 0.3 0 0.1 0.1 0.1 0.1065 0.2 0.2935 0.2 0.3 Hello",
        ]
        ours = "\n".join(lines) + "\n"
        theirs = theirs_file([(100, 200, [("Hello", 10, 21.3, 20, 58.7)])])
        report, ok = self.run_compare(ours, theirs)
        self.assertTrue(ok, report)

    def test_a_split_word_keeps_the_lean_of_its_outermost_part(self):
        # Theirs is Hello as "Hel" and "lo"; ours is one word that leans
        # at both edges. The two halves make a box that is in place with it
        # when its edges lie on the lean.
        ours = ours_file(
            [(200, 100, 0, [("Hello", 0.1, 0.3, 0.3, 0.4, 0, self.LEAN)])]
        )
        theirs = theirs_file(
            [(200, 100, [("Hel", 21.3, 30, 40, 40), ("lo", 40, 30, 58.7, 40)])]
        )
        report, ok = self.run_compare(ours, theirs)
        self.assertTrue(ok, report)
        self.assertIn("split 1", report[0])


if __name__ == "__main__":
    unittest.main()
