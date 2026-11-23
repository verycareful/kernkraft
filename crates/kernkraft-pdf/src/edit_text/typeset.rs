// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Setting new text in the width of the block it replaces: ISO 32000-1:2008,
//! 9.3.2 (`Tc`), 9.3.3 (`Tw`), 9.3.4 (`Tz`) and 9.4.4 (the glyph
//! displacement `tx = ((w0 * Tfs + Tc + Tw) * Th)`).
//!
//! Where a line may end is the Unicode Line Breaking Algorithm, UAX #14, as
//! the tables of `unicode-linebreak` give it (Unicode 15.0): a hard break
//! (LB4, LB5: line feed, carriage return, CR LF, vertical tab, form feed, NEL,
//! U+2028, U+2029) always ends a line; spaces never start one (LB7) and an
//! opportunity after them (LB18) is the usual place to wrap; a no-break
//! space joins its neighbours (LB12, LB12a); a hyphen cannot start a line
//! (LB21); ideographs break between any two (LB31). A run of text between
//! two opportunities moves to the next line whole. One wider than the block
//! stays whole on a line of its own. The spaces that end a line at a wrap
//! are dropped and do not count towards the width; at a hard break or the end
//! of the text they are kept. A soft hyphen (U+00AD) is not text and is
//! dropped, as in the text search.

use std::ops::Range;

use unicode_linebreak::{linebreaks, BreakOpportunity};

use crate::PdfError;

/// What a line is measured with: the horizontal displacement of one
/// character in text space units.
pub(super) type Advance<'a> = dyn Fn(char) -> Result<f64, PdfError> + 'a;

/// The relative error of one addition of two `f64`s is at most
/// [`f64::EPSILON`]; a line's width is a sum of one term for each of its
/// characters, so a line that fills the block exactly, up to the rounding of
/// that sum, is allowed this many `EPSILON` more than the block's width.
fn fit_slack(characters: usize) -> f64 {
    f64::EPSILON * (characters + 1) as f64
}

/// The characters UAX #14 ends a line at, whatever the width (LB4, LB5).
const HARD_BREAKS: [char; 7] = [
    '\n', '\r', '\u{000B}', '\u{000C}', '\u{0085}', '\u{2028}', '\u{2029}',
];

/// The soft hyphen: a break opportunity that shows nothing unless the line
/// breaks there. It is not text here, as in the text search.
const SOFT_HYPHEN: char = '\u{00AD}';

/// The characters of `text` that are drawn: neither a hard break, which ends
/// a line, nor a soft hyphen. What the font has to show is this.
pub(super) fn shown(text: &str) -> String {
    text.chars()
        .filter(|&c| c != SOFT_HYPHEN && !HARD_BREAKS.contains(&c))
        .collect()
}

/// A line of the text set: its characters, and where they are in
/// [`Set::text`].
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Line {
    pub text: String,
    /// The bytes of [`Set::text`] the line is. The characters a wrap or a
    /// hard break drops are outside it.
    pub range: Range<usize>,
}

/// Text broken into lines.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Set {
    /// The text with its soft hyphens taken out, which the lines are
    /// slices of.
    pub text: String,
    pub lines: Vec<Line>,
}

/// `text` broken into lines no wider than `width` (text space units), or
/// not broken but at its hard breaks when `width` is not a positive number.
/// A hard break at the very end of the text leaves an empty last line.
pub(super) fn lines(text: &str, width: f64, advance: &Advance<'_>) -> Result<Set, PdfError> {
    let text: String = text.chars().filter(|&c| c != SOFT_HYPHEN).collect();
    let limit = width * (1.0 + fit_slack(text.chars().count()));
    let mut out: Vec<Line> = Vec::new();
    let mut line = String::new();
    let mut line_start = 0;
    let mut taken = 0.0;
    let mut start = 0;
    for (end, kind) in linebreaks(&text) {
        let segment_start = start;
        let segment = &text[start..end];
        start = end;
        let body = segment.trim_end_matches(HARD_BREAKS);
        let ink = body.trim_end_matches(' ');
        let ink_width = measure(ink, advance)?;
        let line_has_ink = !line.trim_end_matches(' ').is_empty();
        if width > 0.0 && line_has_ink && taken + ink_width > limit {
            let kept = line.trim_end_matches(' ').len();
            line.truncate(kept);
            out.push(Line {
                text: std::mem::take(&mut line),
                range: line_start..line_start + kept,
            });
            taken = 0.0;
        }
        if line.is_empty() {
            line_start = segment_start;
        }
        line.push_str(body);
        taken += ink_width + measure(&body[ink.len()..], advance)?;
        if kind == BreakOpportunity::Mandatory {
            let length = line.len();
            out.push(Line {
                text: std::mem::take(&mut line),
                range: line_start..line_start + length,
            });
            taken = 0.0;
            if end == text.len() && segment.len() != body.len() {
                out.push(Line {
                    text: String::new(),
                    range: end..end,
                });
            }
        }
    }
    if out.is_empty() {
        out.push(Line {
            text: String::new(),
            range: 0..0,
        });
    }
    Ok(Set { text, lines: out })
}

fn measure(text: &str, advance: &Advance<'_>) -> Result<f64, PdfError> {
    text.chars().map(advance).sum()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    /// Every character one unit wide, so a width is a count of characters.
    fn unit(_: char) -> Result<f64, PdfError> {
        Ok(1.0)
    }

    fn wrapped(text: &str, width: f64) -> Vec<String> {
        lines(text, width, &unit)
            .expect("unit advance never fails")
            .lines
            .into_iter()
            .map(|line| line.text)
            .collect()
    }

    #[test]
    fn wraps_after_a_space_lb18() {
        assert_eq!(wrapped("ab cd ef", 5.0), ["ab cd", "ef"]);
    }

    #[test]
    fn a_line_that_fills_the_width_exactly_fits() {
        assert_eq!(wrapped("abcde", 5.0), ["abcde"]);
        assert_eq!(wrapped("ab cd ef", 8.0), ["ab cd ef"]);
    }

    #[test]
    fn the_spaces_that_end_a_wrapped_line_do_not_count_lb7() {
        assert_eq!(wrapped("abcde f", 5.0), ["abcde", "f"]);
        assert_eq!(wrapped("ab   cd", 4.0), ["ab", "cd"]);
    }

    #[test]
    fn spaces_at_the_end_of_the_text_stay_and_start_no_line_lb7() {
        assert_eq!(wrapped("abcde ", 5.0), ["abcde "]);
    }

    #[test]
    fn a_hyphen_stays_on_the_line_it_ends_lb21_lb31() {
        assert_eq!(wrapped("ab-cd", 3.0), ["ab-", "cd"]);
    }

    #[test]
    fn ideographs_break_between_any_two_lb31() {
        assert_eq!(
            wrapped("\u{65E5}\u{672C}\u{8A9E}", 2.0),
            ["\u{65E5}\u{672C}", "\u{8A9E}"]
        );
    }

    #[test]
    fn a_no_break_space_joins_its_neighbours_lb12_lb12a() {
        assert_eq!(wrapped("ab\u{00A0}cd ef", 3.0), ["ab\u{00A0}cd", "ef"]);
        assert_eq!(wrapped("ab\u{00A0}cd ef", 5.0), ["ab\u{00A0}cd", "ef"]);
        assert_eq!(wrapped("ab cd", 3.0), ["ab", "cd"]);
    }

    #[test]
    fn every_hard_break_ends_a_line_lb4_lb5() {
        assert_eq!(wrapped("a\nb", 9.0), ["a", "b"]);
        assert_eq!(wrapped("a\r\nb", 9.0), ["a", "b"]);
        assert_eq!(wrapped("a\rb", 9.0), ["a", "b"]);
        assert_eq!(wrapped("a\u{2028}b", 9.0), ["a", "b"]);
        assert_eq!(wrapped("a\u{000B}b", 9.0), ["a", "b"]);
        assert_eq!(wrapped("a\n\nb", 9.0), ["a", "", "b"]);
    }

    #[test]
    fn a_hard_break_at_the_end_leaves_an_empty_last_line() {
        assert_eq!(wrapped("a\n", 9.0), ["a", ""]);
        assert_eq!(wrapped("a\r\n", 9.0), ["a", ""]);
    }

    #[test]
    fn a_word_wider_than_the_block_stays_whole() {
        assert_eq!(wrapped("ab abcdefgh cd", 4.0), ["ab", "abcdefgh", "cd"]);
    }

    #[test]
    fn leading_spaces_stay_with_the_first_word() {
        assert_eq!(wrapped("   abcdefgh", 4.0), ["   abcdefgh"]);
    }

    #[test]
    fn no_width_breaks_only_at_hard_breaks() {
        assert_eq!(wrapped("ab cd\nef", 0.0), ["ab cd", "ef"]);
        assert_eq!(wrapped("ab cd", -1.0), ["ab cd"]);
    }

    #[test]
    fn a_soft_hyphen_is_dropped() {
        assert_eq!(wrapped("ab\u{00AD}cd", 2.0), ["abcd"]);
    }

    #[test]
    fn a_line_is_the_slice_of_the_text_it_was_cut_from() {
        let set = lines("ab cd\nef  gh", 5.0, &unit).unwrap();
        for line in &set.lines {
            assert_eq!(set.text[line.range.clone()], line.text);
        }
        // `ab cd`, `ef` (the two spaces that end the wrapped line are
        // outside it) and `gh`.
        let ranges: Vec<_> = set.lines.iter().map(|line| line.range.clone()).collect();
        assert_eq!(ranges, [0..5, 6..8, 10..12]);
    }

    #[test]
    fn a_soft_hyphen_is_outside_the_text_the_ranges_are_of() {
        let set = lines("ab\u{00AD}cd", 9.0, &unit).unwrap();
        assert_eq!(set.text, "abcd");
        assert_eq!(set.lines[0].range, 0..4);
    }

    #[test]
    fn empty_text_is_one_empty_line() {
        assert_eq!(wrapped("", 5.0), [""]);
    }

    #[test]
    fn a_failing_advance_is_the_error() {
        let refuse = |c: char| Err(PdfError::NoGlyphForCharacter { character: c });
        assert!(matches!(
            lines("ab", 5.0, &refuse),
            Err(PdfError::NoGlyphForCharacter { character: 'a' })
        ));
    }
}
