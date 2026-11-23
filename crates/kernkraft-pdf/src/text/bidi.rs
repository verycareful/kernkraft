// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The logical order of a line of glyphs that a page shows in visual order:
//! Unicode Standard Annex #9 (UAX #9, the Unicode Bidirectional Algorithm),
//! rules W1 to W7, N0 to N2 and I1 to I2 for the embedding level of each
//! character, and rule L2 for the order of the characters on a line.
//!
//! A page is a set of glyphs placed on it. The layout reads them left to
//! right along their baseline, which is the order a reader sees: a Hebrew
//! word's letters come out last letter first. The text of the page is its
//! logical order, the order the text was typed and is read aloud, and that
//! is what [`logical_order`] gives back. Rule L2 maps logical order to
//! visual order by reversing, from the highest embedding level down to the
//! lowest odd one, every run of characters at that level or above; here
//! the same reversals are applied in the opposite sequence, from the lowest
//! odd level up, which undoes them.
//!
//! The levels come from running the algorithm on the characters as they
//! stand on the line, with the paragraph's base level fixed. That is a
//! reading of the line, not a proof: the same picture can come from a left
//! to right paragraph with a Hebrew phrase inside it and from a right to
//! left one with a Latin phrase at each end, and the rules cannot tell
//! them apart. The base direction is therefore a choice, made in two steps.
//! A producer that draws the glyphs of a line in the order they were typed,
//! each placed where it belongs (Cairo does), has settled it: when exactly
//! one of the two readings of a line is the order its glyphs were drawn in,
//! that reading is taken. Otherwise the line takes the block's base, the
//! direction most of the block's strong characters have ([`base_level`]).
//!
//! The crate does not apply rule L4 (mirroring), and none is applied here:
//! the text of a glyph is what the font's `ToUnicode` map or encoding says
//! it is, which for the producers read so far is the character typed, not
//! the mirrored form drawn.

use unicode_bidi::{bidi_class, BidiClass, BidiInfo, Level};

use crate::text::{TextChar, TextLine};

/// Whether `text` has a character of a right-to-left script: strong class
/// R (Hebrew and the like) or AL (Arabic and the like), UAX #9, table 4.
fn has_right_to_left(text: &str) -> bool {
    text.chars()
        .any(|c| matches!(bidi_class(c), BidiClass::R | BidiClass::AL))
}

/// The base level of a block whose characters have the texts `units`:
/// right to left when its strong right-to-left characters (R and AL) are
/// more than its strong left-to-right ones (L), left to right otherwise.
pub(super) fn base_level<'a>(units: impl Iterator<Item = &'a str>) -> Level {
    let (mut right, mut left) = (0usize, 0usize);
    for c in units.flat_map(str::chars) {
        match bidi_class(c) {
            BidiClass::R | BidiClass::AL => right += 1,
            BidiClass::L => left += 1,
            _ => {}
        }
    }
    if right > left {
        Level::rtl()
    } else {
        Level::ltr()
    }
}

/// The order of the characters of a line that is shown with the texts
/// `units`, left to right, when it is read in logical order: element `k` is
/// the place, counted from the left, of the `k`-th character in reading
/// order. `base` is the paragraph's level. A line with no right-to-left
/// character keeps its order.
pub(super) fn logical_order(units: &[&str], base: Level) -> Vec<usize> {
    if !units.iter().any(|unit| has_right_to_left(unit)) {
        return (0..units.len()).collect();
    }
    let mut text = String::with_capacity(units.iter().map(|unit| unit.len()).sum());
    let mut starts = Vec::with_capacity(units.len());
    for unit in units {
        starts.push(text.len());
        text.push_str(unit);
    }
    let info = BidiInfo::new(&text, Some(base));
    // A character's level is the level of its first scalar value: a
    // ligature the font maps to several is one glyph.
    let levels: Vec<u8> = units
        .iter()
        .zip(&starts)
        .map(|(unit, &start)| match info.levels.get(start) {
            Some(level) if !unit.is_empty() => level.number(),
            _ => base.number(),
        })
        .collect();
    undo_reordering(&levels)
}

/// The places, counted from the left, of the characters of a line in
/// logical order, given the embedding level of each character from the
/// left: UAX #9, rule L2 applied in reverse.
fn undo_reordering(levels: &[u8]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..levels.len()).collect();
    let Some(&highest) = levels.iter().max() else {
        return order;
    };
    let Some(lowest_odd) = levels.iter().copied().filter(|level| level % 2 == 1).min() else {
        return order;
    };
    for level in lowest_odd..=highest {
        let mut at = 0;
        while at < order.len() {
            if levels[order[at]] < level {
                at += 1;
                continue;
            }
            let end = (at..order.len())
                .find(|&index| levels[order[index]] < level)
                .unwrap_or(order.len());
            order[at..end].reverse();
            at = end;
        }
    }
    order
}

/// Where the glyphs of `chars` were drawn in the content stream: for each
/// character the operation, string and offset that drew it, `None` for a
/// space the layout read between glyphs. `None` when the glyphs were not
/// all drawn by one stream's one invocation of it, where the places of two
/// cannot be compared.
fn drawing_places(chars: &[TextChar]) -> Option<Vec<Option<(u32, u32, u32)>>> {
    let mut sources = chars.iter().filter_map(|ch| ch.source);
    let first = sources.next()?;
    if sources.any(|source| {
        source.span.stream != first.span.stream || source.invocation != first.invocation
    }) {
        return None;
    }
    Some(
        chars
            .iter()
            .map(|ch| {
                ch.source
                    .map(|source| (source.span.index, source.operand, source.offset))
            })
            .collect(),
    )
}

/// Whether the characters, read in `order` (the place of each along the
/// line, in reading order), come in the order they were drawn in.
fn is_drawing_order(places: &[Option<(u32, u32, u32)>], order: &[usize]) -> bool {
    let mut drawn = order.iter().filter_map(|&place| places[place]);
    let Some(mut before) = drawn.next() else {
        return true;
    };
    drawn.all(|place| {
        let in_order = before < place;
        before = place;
        in_order
    })
}

/// The order of a line's characters in reading order, as [`logical_order`]
/// gives it for `base`, unless the other base gives the order the glyphs
/// were drawn in and `base` does not.
fn reading_order(line: &TextLine, units: &[&str], base: Level) -> Vec<usize> {
    let order = logical_order(units, base);
    let Some(places) = drawing_places(&line.chars) else {
        return order;
    };
    if is_drawing_order(&places, &order) {
        return order;
    }
    let other = logical_order(
        units,
        if base.is_rtl() {
            Level::ltr()
        } else {
            Level::rtl()
        },
    );
    if is_drawing_order(&places, &other) {
        other
    } else {
        order
    }
}

/// Puts the characters of each line of a block, given left to right, in
/// logical order. Each keeps the box where it is drawn, and the line
/// remembers where each one stands on it.
///
/// A line's base level is the one its glyphs' drawing order settles when it
/// does ([`reading_order`]) and otherwise the block's, that of the
/// direction most of the block's strong characters have ([`base_level`]); a
/// line with no right-to-left character is left as it is, whatever the
/// block's base.
pub(super) fn in_logical_order(lines: &mut [TextLine]) {
    let base = base_level(
        lines
            .iter()
            .flat_map(|line| line.chars.iter())
            .filter(|ch| !ch.synthetic)
            .map(|ch| ch.text.as_str()),
    );
    for line in lines {
        let units: Vec<&str> = line.chars.iter().map(|ch| ch.text.as_str()).collect();
        let order = reading_order(line, &units, base);
        if order.iter().enumerate().all(|(k, &place)| k == place) {
            continue;
        }
        let mut rank = vec![0; order.len()];
        for (k, &place) in order.iter().enumerate() {
            rank[place] = k;
        }
        let mut keyed: Vec<(usize, _)> = std::mem::take(&mut line.chars)
            .into_iter()
            .enumerate()
            .map(|(place, ch)| (rank[place], ch))
            .collect();
        keyed.sort_by_key(|(k, _)| *k);
        line.chars = keyed.into_iter().map(|(_, ch)| ch).collect();
        line.visual = order;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    /// The Hebrew letters the cases are set in.
    const SHIN: &str = "\u{05e9}";
    const LAMED: &str = "\u{05dc}";
    const VAV: &str = "\u{05d5}";
    const FINAL_MEM: &str = "\u{05dd}";
    const AYIN: &str = "\u{05e2}";

    /// The characters of `visual`, left to right, read in logical order
    /// under `base`.
    fn read(visual: &[&str], base: Level) -> String {
        logical_order(visual, base)
            .into_iter()
            .map(|place| visual[place])
            .collect()
    }

    #[test]
    fn a_line_without_a_right_to_left_character_keeps_its_order() {
        let line = ["a", "b", "c", " ", "1", "2"];
        assert_eq!(
            logical_order(&line, Level::ltr()),
            (0..line.len()).collect::<Vec<_>>()
        );
        assert_eq!(
            logical_order(&line, Level::rtl()),
            (0..line.len()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_hebrew_word_in_a_left_to_right_line_is_read_from_its_other_end() {
        // Typed `abc` + shalom + `def`, shown with the word's letters
        // reversed, mem first: L L L space R R R R space L L L, levels 0 for
        // the Latin and the spaces (N1 does not apply between L and R, so N2
        // gives the embedding level 0) and 1 for the word.
        let visual = [
            "a", "b", "c", " ", FINAL_MEM, VAV, LAMED, SHIN, " ", "d", "e", "f",
        ];
        assert_eq!(
            read(&visual, Level::ltr()),
            format!("abc {SHIN}{LAMED}{VAV}{FINAL_MEM} def")
        );
    }

    #[test]
    fn a_right_to_left_line_is_read_from_its_right_end() {
        // Typed shalom + space + olam in a right to left paragraph, shown
        // from the left: olam reversed, space, shalom reversed. Every
        // character is at level 1, so the whole line is reversed.
        let visual = [
            FINAL_MEM, LAMED, VAV, AYIN, " ", FINAL_MEM, VAV, LAMED, SHIN,
        ];
        assert_eq!(
            read(&visual, Level::rtl()),
            format!("{SHIN}{LAMED}{VAV}{FINAL_MEM} {AYIN}{VAV}{LAMED}{FINAL_MEM}")
        );
    }

    #[test]
    fn latin_at_both_ends_of_a_right_to_left_line_keeps_its_letters_in_order() {
        // Typed `def` + shalom + olam + `abc` in a right to left paragraph:
        // `abc` is at the left end and `def` at the right. The Latin runs
        // are level 2 and the rest level 1 (N2 between L and R gives the
        // embedding level 1). Reversing everything at level 1 and above
        // puts `def` first; reversing each level 2 run restores its letters.
        let visual = [
            "a", "b", "c", " ", FINAL_MEM, LAMED, VAV, AYIN, " ", FINAL_MEM, VAV, LAMED, SHIN, " ",
            "d", "e", "f",
        ];
        assert_eq!(
            read(&visual, Level::rtl()),
            format!("def {SHIN}{LAMED}{VAV}{FINAL_MEM} {AYIN}{VAV}{LAMED}{FINAL_MEM} abc")
        );
    }

    #[test]
    fn european_digits_in_a_right_to_left_line_keep_their_order() {
        // Typed shalom + space + `2026` in a right to left paragraph, shown
        // as `2026` (level 2, a number: W7 leaves EN as EN after sos R),
        // a space (N1: between EN, which counts as R, and R, so R, level 1)
        // and the word reversed (level 1).
        let visual = ["2", "0", "2", "6", " ", FINAL_MEM, VAV, LAMED, SHIN];
        assert_eq!(
            read(&visual, Level::rtl()),
            format!("{SHIN}{LAMED}{VAV}{FINAL_MEM} 2026")
        );
    }

    #[test]
    fn brackets_around_a_right_to_left_word_are_read_in_the_order_typed() {
        // Typed `(` shalom `)`: the producer draws mirrored glyphs whose
        // text is still the character typed, so from the left the line is
        // `)`, the word reversed, `(`. All three are level 1 (N1: between
        // sos R and R, and between R and eos R).
        let visual = [")", FINAL_MEM, VAV, LAMED, SHIN, "("];
        assert_eq!(
            read(&visual, Level::rtl()),
            format!("({SHIN}{LAMED}{VAV}{FINAL_MEM})")
        );
    }

    #[test]
    fn a_character_of_several_scalar_values_takes_the_level_of_its_first() {
        // A ligature glyph `fi` between Hebrew letters in a right to left
        // line: its first scalar value is L, so it is level 2 on its own.
        let visual = [FINAL_MEM, "fi", SHIN];
        assert_eq!(read(&visual, Level::rtl()), format!("{SHIN}fi{FINAL_MEM}"));
    }

    #[test]
    fn the_base_is_the_direction_most_strong_characters_have() {
        let hebrew = [SHIN, LAMED, VAV, FINAL_MEM];
        let latin = ["a", "b", "c"];
        assert_eq!(
            base_level(hebrew.iter().chain(latin.iter()).copied()),
            Level::rtl()
        );
        assert_eq!(
            base_level(hebrew[..2].iter().chain(latin.iter()).copied()),
            Level::ltr()
        );
        // A tie and a text with no strong character are left to right.
        assert_eq!(
            base_level(hebrew[..3].iter().chain(latin.iter()).copied()),
            Level::ltr()
        );
        assert_eq!(base_level([" ", "1"].into_iter()), Level::ltr());
    }

    #[test]
    fn undoing_the_reordering_inverts_rule_l2_for_every_short_run_of_levels() {
        // For every sequence of up to six levels from 0 to 3 in logical
        // order, `reorder_visual` (rule L2) gives the visual order; the
        // levels in that order, read back, must give the logical order.
        for length in 1..=6usize {
            for code in 0..4usize.pow(length as u32) {
                let logical: Vec<u8> = (0..length).map(|i| ((code >> (2 * i)) & 3) as u8).collect();
                let levels: Vec<Level> = logical.iter().map(|&n| Level::new(n).unwrap()).collect();
                let visual_to_logical = BidiInfo::reorder_visual(&levels);
                let visual_levels: Vec<u8> =
                    visual_to_logical.iter().map(|&k| logical[k]).collect();
                let order = undo_reordering(&visual_levels);
                // Element k of `order` is the visual place of logical
                // character k.
                for (visual, &k) in visual_to_logical.iter().enumerate() {
                    assert_eq!(order[k], visual, "levels {logical:?}");
                }
            }
        }
    }
}
