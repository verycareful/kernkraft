// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The logical order of right-to-left and mixed text: `text/bidi.rs`, used
//! by `text/layout.rs`. Unicode Standard Annex #9, rules W1 to W7, N0 to
//! N2, I1 to I2 and L2.
//!
//! The font is Courier with its codes 1 to 5 named for five Hebrew letters
//! (`uniXXXX` names, Adobe Glyph List specification, section 2), so every
//! glyph advances 600 units, 6 points at 10 points, as in the other text
//! tests. A producer that draws a right-to-left word left to right shows
//! its last letter first, so a word is written below with its letters
//! reversed.

use super::*;
use kernkraft_pdf::text::SearchOptions;

/// The font object: the letters are code 1 shin, 2 lamed, 3 vav, 4 final
/// mem, 5 ayin, so shalom is shin lamed vav mem (codes 1 2 3 4) and olam is
/// ayin vav lamed mem (5 3 2 4). A name outside the Core 14 metrics has no
/// width of its own, so every code from 1 to 255 is given 600 units.
fn hebrew_font() -> (u32, Vec<u8>) {
    let widths = "600 ".repeat(255);
    (
        80,
        format!(
            "<< /Type /Font /Subtype /Type1 /BaseFont /Courier \
             /Encoding << /BaseEncoding /WinAnsiEncoding \
             /Differences [1 /uni05E9 /uni05DC /uni05D5 /uni05DD /uni05E2] >> \
             /FirstChar 1 /LastChar 255 /Widths [{widths}] >>"
        )
        .into_bytes(),
    )
}

const SHALOM: &str = "\u{05e9}\u{05dc}\u{05d5}\u{05dd}";
const OLAM: &str = "\u{05e2}\u{05d5}\u{05dc}\u{05dd}";

/// The text layer of a page that shows `hex` (the code string of one `Tj`)
/// at (100, 700) in the Hebrew font, and `more` after it.
fn drawn(hex: &str, more: &str) -> TextPage {
    let content = format!("BT /F3 10 Tf 100 700 Td <{hex}> Tj {more} ET");
    let fixture = document(
        &[PageSpec {
            content: &content,
            entries: "/Resources << /Font << /F3 80 0 R >> >>",
        }],
        "",
        "",
        &[(80, hebrew_font().1.as_slice())],
    );
    text_of(&fixture)
}

/// The codes of `abc ` and the like in the Hebrew font: its Latin part is
/// WinAnsi, so the code of a letter is its ASCII value.
fn ascii(text: &str) -> String {
    text.bytes().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn a_hebrew_word_drawn_last_letter_first_is_read_first_letter_first() {
    // Mem vav lamed shin from the left, x 100 to 124.
    let page = drawn("04030201", "");
    assert_eq!(page.text(), SHALOM);
    let chars = &page.blocks[0].lines[0].chars;
    // Each character keeps the box where it is drawn: shin, the first in
    // reading order, is the fourth glyph, x 118 to 124; mem is the first.
    assert_rect(chars[0].rect, 118.0, 84.0, 124.0, 94.0, "shin");
    assert_rect(chars[3].rect, 100.0, 84.0, 106.0, 94.0, "mem");
}

#[test]
fn a_hebrew_phrase_inside_a_left_to_right_line_keeps_the_latin_on_the_left() {
    // Typed `abc `, shalom, ` def`; the word is drawn reversed. Four
    // strong right-to-left characters against six left-to-right ones make
    // the paragraph left to right.
    let page = drawn(&format!("{}04030201{}", ascii("abc "), ascii(" def")), "");
    assert_eq!(page.text(), format!("abc {SHALOM} def"));
}

#[test]
fn a_right_to_left_line_with_latin_at_its_ends_is_read_from_the_right() {
    // Typed `def `, shalom, olam, ` abc` in a right to left paragraph
    // (eight strong right-to-left characters against six left-to-right
    // ones), drawn from the left as `abc `, olam reversed, a space, shalom
    // reversed and ` def`.
    let hex = format!(
        "{}04020305{}04030201{}",
        ascii("abc "),
        ascii(" "),
        ascii(" def")
    );
    let page = drawn(&hex, "");
    assert_eq!(page.text(), format!("def {SHALOM} {OLAM} abc"));
}

#[test]
fn european_digits_after_a_right_to_left_word_stay_in_order() {
    // Typed shalom, a space and `2026` in a right to left paragraph, drawn
    // from the left as `2026`, a space and shalom reversed.
    let page = drawn(&format!("{}2004030201", ascii("2026")), "");
    assert_eq!(page.text(), format!("{SHALOM} 2026"));
}

#[test]
fn a_line_of_latin_beside_a_right_to_left_line_is_not_reordered() {
    // Two lines of one block, the first Hebrew and the second `abc.`: the
    // second has no right-to-left character, so its full stop stays last
    // although the block is right to left.
    let page = drawn("0403020104030201", "0 -12 Td (abc.) Tj");
    // The second line's text is Latin in the Hebrew font's WinAnsi part.
    assert_eq!(page.blocks.len(), 1);
    assert_eq!(page.blocks[0].lines.len(), 2);
    assert_eq!(page.blocks[0].lines[0].text(), format!("{SHALOM}{SHALOM}"));
    assert_eq!(page.blocks[0].lines[1].text(), "abc.");
}

#[test]
fn a_selection_in_mixed_text_covers_each_stretch_it_selects_and_no_more() {
    // `abc `, shalom reversed, ` def`: from the left a b c space mem vav
    // lamed shin space d e f, characters 0 to 11 along the line. In
    // reading order the characters are a b c space shin lamed vav mem
    // space d e f; selecting `c`, the space and shin (reading order 2 to
    // 4) covers places 2 and 3, x 112 to 124, and place 7, x 142 to 148.
    let page = drawn(&format!("{}04030201{}", ascii("abc "), ascii(" def")), "");
    let selection = page.selection(2, 4).unwrap();
    assert_eq!(selection.text, "c \u{05e9}");
    assert_eq!(selection.rects.len(), 2);
    assert_rect(selection.rects[0], 112.0, 84.0, 124.0, 94.0, "c and space");
    assert_rect(selection.rects[1], 142.0, 84.0, 148.0, 94.0, "shin");
}

#[test]
fn a_search_finds_a_hebrew_word_by_its_reading_order_and_boxes_it_where_it_is_drawn() {
    let page = drawn(&format!("{}04030201{}", ascii("abc "), ascii(" def")), "");
    let matches = page.search(SHALOM, &SearchOptions::default());
    assert_eq!(matches.len(), 1);
    // The word is places 4 to 7: x 124 to 148.
    assert_eq!(matches[0].rects.len(), 1);
    assert_rect(matches[0].rects[0], 124.0, 84.0, 148.0, 94.0, "shalom");
    // The word drawn in the other order is not there.
    let reversed: String = SHALOM.chars().rev().collect();
    assert!(page.search(&reversed, &SearchOptions::default()).is_empty());
}

/// The text layer of a page that shows each of `glyphs` by itself: its
/// code and the place along the line, counted from 0, where it stands, in
/// 6 point steps from x 100, at 10 points. The glyphs are drawn in the
/// order given.
fn placed(glyphs: &[(u8, u32)]) -> TextPage {
    let mut content = String::from("BT /F3 10 Tf ");
    for &(code, place) in glyphs {
        content.push_str(&format!(
            "1 0 0 1 {} 700 Tm <{code:02x}> Tj ",
            100 + 6 * place
        ));
    }
    content.push_str("ET");
    let fixture = document(
        &[PageSpec {
            content: &content,
            entries: "/Resources << /Font << /F3 80 0 R >> >>",
        }],
        "",
        "",
        &[(80, hebrew_font().1.as_slice())],
    );
    text_of(&fixture)
}

/// The line `def `, a Hebrew phrase, ` abc` as a left to right paragraph
/// shows it: `def ` at places 0 to 3, then the phrase shalom, a space,
/// olam reversed as one run (mem lamed vav ayin, a space, mem vav lamed
/// shin) at places 4 to 12, then ` abc` at places 13 to 16. The glyphs are
/// listed in the order they were typed.
fn typed_left_to_right() -> Vec<(u8, u32)> {
    let ascii = |text: &str, from: u32| -> Vec<(u8, u32)> { text.bytes().zip(from..).collect() };
    let mut glyphs = ascii("def ", 0);
    // Shalom: shin lamed vav mem is drawn at places 12, 11, 10 and 9.
    glyphs.extend([(1, 12), (2, 11), (3, 10), (4, 9)]);
    glyphs.push((b' ', 8));
    // Olam: ayin vav lamed mem is drawn at places 7, 6, 5 and 4.
    glyphs.extend([(5, 7), (3, 6), (2, 5), (4, 4)]);
    glyphs.extend(ascii(" abc", 13));
    glyphs
}

#[test]
fn glyphs_drawn_in_the_order_typed_settle_a_left_to_right_paragraph() {
    // Eight right-to-left characters against six left-to-right ones would
    // make the paragraph right to left and read `abc` first. The glyphs
    // were drawn `def`, shalom, olam, `abc`, which is the left to right
    // reading and not the right to left one.
    let page = placed(&typed_left_to_right());
    assert_eq!(page.text(), format!("def {SHALOM} {OLAM} abc"));
}

#[test]
fn glyphs_drawn_along_the_line_leave_the_base_to_the_majority() {
    // The same picture drawn from the left: the drawing order is neither
    // reading, so the eight right-to-left characters against six decide:
    // a right to left paragraph, read from its right end.
    let mut glyphs = typed_left_to_right();
    glyphs.sort_by_key(|&(_, place)| place);
    let page = placed(&glyphs);
    assert_eq!(page.text(), format!("abc {SHALOM} {OLAM} def"));
}
