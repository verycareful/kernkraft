// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Which lines of a block ended by a wrap: `text/wrap.rs`.
//!
//! The text is Courier at 10 points, a glyph and a space 6 points wide, set
//! 12 points apart so that its lines are one block. A column of 30
//! characters is 180 points; a line wraps when the next word, with a space,
//! would not have fitted in what is left of it.

use super::*;

/// The lines at (100, 700), 12 points apart, in one text object.
fn set(lines: &[&str]) -> TextPage {
    let mut content = String::from("BT /F1 10 Tf 100 700 Td");
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            content.push_str(" 0 -12 Td");
        }
        let escaped = line.replace('(', "\\(").replace(')', "\\)");
        content.push_str(&format!(" ({escaped}) Tj"));
    }
    content.push_str(" ET");
    page(&content)
}

fn wrapped(lines: &[&str]) -> Vec<bool> {
    let page = set(lines);
    assert_eq!(page.blocks.len(), 1, "one block");
    assert_eq!(page.blocks[0].lines.len(), lines.len());
    page.blocks[0].wrapped_lines()
}

#[test]
fn the_lines_of_a_paragraph_wrapped_to_a_column_are_wraps_and_its_last_is_not() {
    // Set greedily to 30 characters: "lambda" (6, with its space 7) does
    // not fit the 5 characters left after the second line's 25, and "zeta"
    // does not fit the 0 left after the first's 30.
    let lines = [
        "alpha beta gamma delta epsilon",
        "zeta eta theta iota kappa",
        "lambda mu.",
    ];
    assert_eq!(wrapped(&lines), vec![true, true, false]);
}

#[test]
fn a_line_that_stops_where_the_next_word_would_have_fitted_is_a_break() {
    // 7 characters of room against "dolor" and a space: it fits, so the
    // writer ended the line.
    let lines = ["lorem ipsum", "dolor sit amet, consectetur"];
    assert_eq!(wrapped(&lines), vec![false, false]);
}

#[test]
fn an_address_is_lines_though_one_of_them_reaches_the_edge() {
    // The second line is the longest and the next word does not fit after
    // it, which on its own reads as a wrap; the first line, with room for
    // "12", shows the writer ended it, and the block is read as lines.
    let lines = ["Jane Doe", "12 Long Street Name", "Springfield"];
    assert_eq!(wrapped(&lines), vec![false, false, false]);
}

#[test]
fn a_list_is_lines_whatever_their_lengths() {
    // "- apples" is one character short of "- oranges": the next word, a
    // dash, does not fit in 6 points beside its space, so only the marker
    // tells the lines apart.
    assert_eq!(
        wrapped(&["- apples", "- oranges", "- figs"]),
        vec![false, false, false]
    );
    assert_eq!(
        wrapped(&["1. apples", "2. oranges", "3. figs"]),
        vec![false, false, false]
    );
    assert_eq!(
        wrapped(&["a) apples", "b) oranges", "c) figs"]),
        vec![false, false, false]
    );
}

#[test]
fn a_marker_is_a_lone_sign_or_a_number_or_letter_closed_by_a_stop_or_parenthesis() {
    // "etc." is not one, so a line before it that reaches the edge wraps.
    assert_eq!(
        wrapped(&["alpha beta gamma delta epsilon", "etc. etc."]),
        vec![true, false]
    );
    // "12." is.
    assert_eq!(
        wrapped(&["alpha beta gamma delta epsilon", "12. etc."]),
        vec![false, false]
    );
}

#[test]
fn a_block_of_one_line_has_no_wrap() {
    assert_eq!(wrapped(&["alpha beta gamma delta epsilon"]), vec![false]);
}

#[test]
fn a_block_of_two_lines_whose_first_is_longer_looks_like_a_paragraph() {
    // Nothing in the geometry tells it from a paragraph of two lines.
    assert_eq!(
        wrapped(&["12 Long Street Name", "Springfield"]),
        vec![true, false]
    );
}

#[test]
fn a_page_turned_a_quarter_has_no_wrap() {
    let fixture = document(
        &[PageSpec {
            content: "BT /F1 10 Tf 100 700 Td (alpha beta gamma delta epsilon) Tj 0 -12 Td \
                      (zeta eta theta iota kappa) Tj 0 -12 Td (lambda mu.) Tj ET",
            entries: "/Rotate 90",
        }],
        "",
        "",
        &[],
    );
    let page = text_of(&fixture);
    assert_eq!(page.blocks.len(), 1);
    assert_eq!(page.blocks[0].wrapped_lines(), vec![false, false, false]);
}
