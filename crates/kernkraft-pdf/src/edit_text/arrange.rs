// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Setting a line of new text in the order it is shown in: Unicode Standard
//! Annex #9 (UAX #9), P2 and P3 (the level of a paragraph), W1 to W7, N0 to
//! N2 and I1 to I2 (the level of a character), L1 and L2 (the order of a
//! line) and L4 (mirroring, which is refused: see
//! [`PdfError::MirroredGlyphNotSet`]).
//!
//! The text of an edit is in logical order, the order it was typed in. A
//! line is broken in that order ([`super::typeset`]), and each broken line
//! is then shown in the order its levels give. The glyphs are not written
//! in that order, though: they are written in logical order, each piece
//! placed where it stands, which is how Cairo writes a page and what lets
//! [`crate::text`] read a line back in its own order from the order its
//! glyphs were drawn in. A piece is a run of characters that stand side by
//! side in the order they were typed in, so a line of left-to-right text is
//! one piece, and a right-to-left word is a piece for each letter.

use std::ops::Range;

use unicode_bidi::{BidiInfo, Level};

use super::mirrored_table::BIDI_MIRRORED;
use super::typeset::{Advance, Set};
use crate::PdfError;

/// A run of characters shown side by side in the order they were typed in.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Piece {
    pub text: String,
    /// The distance from the left edge of the line to the piece's left
    /// edge, in text space units.
    pub left: f64,
}

/// A line as it is shown.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Arranged {
    /// The pieces in the order the characters were typed in.
    pub pieces: Vec<Piece>,
    /// The width of the line, in text space units.
    pub width: f64,
    /// Whether the line's paragraph is right to left, so that the line is
    /// set against the right edge of the block.
    pub right_to_left: bool,
}

/// Whether `c` is drawn mirrored where its direction is right to left: it
/// has the Bidi_Mirrored property.
fn is_mirrored(c: char) -> bool {
    let code = u32::from(c);
    BIDI_MIRRORED
        .binary_search_by(|&(first, last)| {
            if code < first {
                std::cmp::Ordering::Greater
            } else if code > last {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// The pieces of the line of `set` that is `range` of `set.text`, in the
/// paragraph `levels` and `info` give.
fn pieces_of(
    info: &BidiInfo<'_>,
    paragraph: &unicode_bidi::ParagraphInfo,
    text: &str,
    range: Range<usize>,
    advance: &Advance<'_>,
) -> Result<(Vec<Piece>, f64), PdfError> {
    let levels = info.reordered_levels(paragraph, range.clone());
    let chars: Vec<(char, Level)> = text
        .char_indices()
        .map(|(offset, c)| (c, levels[range.start + offset]))
        .collect();
    if let Some(&(character, _)) = chars
        .iter()
        .find(|(c, level)| level.is_rtl() && is_mirrored(*c))
    {
        return Err(PdfError::MirroredGlyphNotSet { character });
    }
    let level_list: Vec<Level> = chars.iter().map(|&(_, level)| level).collect();
    // `visual_to_logical[place]` is the character shown at `place`.
    let visual_to_logical = BidiInfo::reorder_visual(&level_list);
    let mut place_of = vec![0usize; chars.len()];
    let mut left_of = vec![0.0f64; chars.len()];
    let mut width = 0.0;
    for (place, &logical) in visual_to_logical.iter().enumerate() {
        place_of[logical] = place;
        left_of[logical] = width;
        width += advance(chars[logical].0)?;
    }
    let mut pieces: Vec<Piece> = Vec::new();
    for (logical, &(c, _)) in chars.iter().enumerate() {
        let continues = logical > 0 && place_of[logical] == place_of[logical - 1] + 1;
        match pieces.last_mut() {
            Some(piece) if continues => piece.text.push(c),
            _ => pieces.push(Piece {
                text: c.to_string(),
                left: left_of[logical],
            }),
        }
    }
    Ok((pieces, width))
}

/// The lines of `set`, each as it is shown. A paragraph's level is its
/// first strong character's (P2, P3).
///
/// [`PdfError::MirroredGlyphNotSet`] for a character that is mirrored in a
/// right-to-left run.
pub(super) fn arrange(set: &Set, advance: &Advance<'_>) -> Result<Vec<Arranged>, PdfError> {
    let info = BidiInfo::new(&set.text, None);
    let mut arranged = Vec::with_capacity(set.lines.len());
    for line in &set.lines {
        let paragraph = info
            .paragraphs
            .iter()
            .rfind(|paragraph| paragraph.range.start <= line.range.start);
        let (pieces, width, right_to_left) = match paragraph {
            Some(paragraph) if info.has_rtl() => {
                let (pieces, width) =
                    pieces_of(&info, paragraph, &line.text, line.range.clone(), advance)?;
                (pieces, width, paragraph.level.is_rtl())
            }
            _ => {
                let width = line.text.chars().map(advance).sum::<Result<f64, _>>()?;
                let pieces = vec![Piece {
                    text: line.text.clone(),
                    left: 0.0,
                }];
                (pieces, width, false)
            }
        };
        arranged.push(Arranged {
            pieces,
            width,
            right_to_left,
        });
    }
    Ok(arranged)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::edit_text::typeset::lines;

    const SHIN: &str = "\u{05e9}";
    const LAMED: &str = "\u{05dc}";
    const VAV: &str = "\u{05d5}";
    const FINAL_MEM: &str = "\u{05dd}";

    /// Every character one unit wide.
    fn unit(_: char) -> Result<f64, PdfError> {
        Ok(1.0)
    }

    fn arranged(text: &str, width: f64) -> Vec<Arranged> {
        arrange(&lines(text, width, &unit).unwrap(), &unit).unwrap()
    }

    fn pieces(line: &Arranged) -> Vec<(&str, f64)> {
        line.pieces
            .iter()
            .map(|piece| (piece.text.as_str(), piece.left))
            .collect()
    }

    #[test]
    fn a_line_of_latin_is_one_piece_at_the_left() {
        let all = arranged("abc def", 99.0);
        assert_eq!(pieces(&all[0]), [("abc def", 0.0)]);
        assert_eq!(all[0].width, 7.0);
        assert!(!all[0].right_to_left);
    }

    #[test]
    fn a_hebrew_word_is_a_piece_for_each_letter_from_the_right() {
        // Shin lamed vav mem, typed in that order, is level 1 in a
        // paragraph that starts with a strong right-to-left character
        // (P2, P3), so the line is reversed: shin is the rightmost of four
        // places and mem the leftmost.
        let text = format!("{SHIN}{LAMED}{VAV}{FINAL_MEM}");
        let all = arranged(&text, 99.0);
        assert_eq!(
            pieces(&all[0]),
            [(SHIN, 3.0), (LAMED, 2.0), (VAV, 1.0), (FINAL_MEM, 0.0)]
        );
        assert_eq!(all[0].width, 4.0);
        assert!(all[0].right_to_left);
    }

    #[test]
    fn a_hebrew_phrase_in_a_latin_line_keeps_the_latin_in_one_piece_each() {
        // `abc `, a word, ` def`: places 0 to 3 are `abc `, the word's
        // letters fill 4 to 7 reversed, and ` def` is 8 to 11 (levels 0, 1,
        // 0; the spaces take the paragraph's level, N2).
        let text = format!("abc {SHIN}{LAMED}{VAV}{FINAL_MEM} def");
        let all = arranged(&text, 99.0);
        assert_eq!(
            pieces(&all[0]),
            [
                ("abc ", 0.0),
                (SHIN, 7.0),
                (LAMED, 6.0),
                (VAV, 5.0),
                (FINAL_MEM, 4.0),
                (" def", 8.0)
            ]
        );
        assert!(!all[0].right_to_left);
    }

    #[test]
    fn digits_in_a_right_to_left_line_stay_side_by_side() {
        // Shalom, a space and `2026`: levels 1, 1, 2 (W, I2). The line
        // reads from the right: `2026` is at places 0 to 3 as one piece,
        // the space is at 4 and the word reversed at 5 to 8.
        let text = format!("{SHIN}{LAMED}{VAV}{FINAL_MEM} 2026");
        let all = arranged(&text, 99.0);
        assert_eq!(
            pieces(&all[0]),
            [
                (SHIN, 8.0),
                (LAMED, 7.0),
                (VAV, 6.0),
                (FINAL_MEM, 5.0),
                (" ", 4.0),
                ("2026", 0.0)
            ]
        );
    }

    #[test]
    fn each_wrapped_line_is_arranged_by_itself_in_its_paragraphs_level() {
        // Two Hebrew words that wrap: each line is its own width, both in
        // the right-to-left paragraph.
        let word = format!("{SHIN}{LAMED}{VAV}{FINAL_MEM}");
        let all = arranged(&format!("{word} {word}"), 4.0);
        assert_eq!(all.len(), 2);
        assert!(all
            .iter()
            .all(|line| line.right_to_left && line.width == 4.0));
    }

    #[test]
    fn a_bracket_in_a_right_to_left_run_is_refused() {
        let text = format!("({SHIN}{LAMED})");
        let set = lines(&text, 99.0, &unit).unwrap();
        assert!(matches!(
            arrange(&set, &unit),
            Err(PdfError::MirroredGlyphNotSet { character: '(' })
        ));
    }

    #[test]
    fn a_bracket_in_a_left_to_right_run_is_set() {
        let all = arranged("(abc)", 99.0);
        assert_eq!(pieces(&all[0]), [("(abc)", 0.0)]);
    }

    #[test]
    fn the_table_has_the_brackets_and_not_the_letters() {
        for c in [
            '(', ')', '[', ']', '{', '}', '<', '>', '\u{00ab}', '\u{00bb}',
        ] {
            assert!(is_mirrored(c), "{c}");
        }
        for c in ['a', '1', ' ', '\u{05d0}'] {
            assert!(!is_mirrored(c), "{c}");
        }
    }
}
