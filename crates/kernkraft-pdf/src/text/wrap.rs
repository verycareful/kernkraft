// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Which lines of a block ended where the text wrapped, and which where its
//! writer broke the line. A PDF records neither: only where each glyph was
//! drawn is known, so this is a judgement from the geometry.
//!
//! A wrapped line stops short of the block's end edge by less than the first
//! word of the next line would take (that word did not fit, or the writer
//! would have kept it there); a line that stops short by that much or more
//! was ended by its writer. A block is judged as a whole: one line that
//! was certainly ended by its writer (the next word would have fitted, or
//! the next line starts with a list marker) makes it a block of lines, such
//! as an address or a list, and none of its lines is called a wrap.

use super::{TextBlock, TextChar, TextLine};

/// The way a line's text runs on the page, from its first character to its
/// last.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Flow {
    LeftToRight,
    RightToLeft,
}

/// Which lines of `block` ended by a wrap; see [`TextBlock::wrapped_lines`].
pub(super) fn wrapped_lines(block: &TextBlock) -> Vec<bool> {
    let lines = &block.lines;
    let none = vec![false; lines.len()];
    let Some(flow) = lines.first().and_then(flow_of) else {
        return none;
    };
    if lines.iter().any(|line| flow_of(line) != Some(flow)) {
        return none;
    }
    let edge = match flow {
        Flow::LeftToRight => lines
            .iter()
            .map(|line| line.rect.right)
            .fold(f64::NEG_INFINITY, f64::max),
        Flow::RightToLeft => lines
            .iter()
            .map(|line| line.rect.left)
            .fold(f64::INFINITY, f64::min),
    };
    let mut wrapped = none;
    for (index, pair) in lines.windows(2).enumerate() {
        let (line, next) = (&pair[0], &pair[1]);
        let word = first_word(next);
        if word.is_empty() || is_list_marker(&text_of(&word)) {
            return vec![false; lines.len()];
        }
        let slack = match flow {
            Flow::LeftToRight => edge - line.rect.right,
            Flow::RightToLeft => line.rect.left - edge,
        };
        if slack >= extent_of(&word) + space_width(next, line) {
            return vec![false; lines.len()];
        }
        wrapped[index] = true;
    }
    wrapped
}

/// The way `line` runs, or `None` when it does not run along the page's
/// horizontal (a vertical line, a turned one, or one of a single glyph).
fn flow_of(line: &TextLine) -> Option<Flow> {
    let mut drawn = line.chars.iter().filter(|ch| !ch.synthetic && !ch.sizeless);
    let first = drawn.next()?;
    let last = drawn.next_back()?;
    let across = (last.rect.left + last.rect.right - first.rect.left - first.rect.right) / 2.0;
    let down = (last.rect.top + last.rect.bottom - first.rect.top - first.rect.bottom) / 2.0;
    if across.abs() <= down.abs() {
        None
    } else if across > 0.0 {
        Some(Flow::LeftToRight)
    } else {
        Some(Flow::RightToLeft)
    }
}

fn is_space(ch: &TextChar) -> bool {
    ch.text.chars().all(char::is_whitespace)
}

/// The characters of the first word of `line`, in reading order.
fn first_word(line: &TextLine) -> Vec<&TextChar> {
    line.chars
        .iter()
        .skip_while(|ch| is_space(ch))
        .take_while(|ch| !is_space(ch))
        .collect()
}

fn text_of(word: &[&TextChar]) -> String {
    word.iter().map(|ch| ch.text.as_str()).collect()
}

/// How far along the page the characters of `word` reach.
fn extent_of(word: &[&TextChar]) -> f64 {
    let left = word
        .iter()
        .map(|ch| ch.rect.left)
        .fold(f64::INFINITY, f64::min);
    let right = word
        .iter()
        .map(|ch| ch.rect.right)
        .fold(f64::NEG_INFINITY, f64::max);
    right - left
}

/// The width of a space where the text is set: the first one in `next`,
/// else the first in `line`, else none.
fn space_width(next: &TextLine, line: &TextLine) -> f64 {
    next.chars
        .iter()
        .chain(&line.chars)
        .find(|ch| is_space(ch))
        .map_or(0.0, |ch| ch.rect.right - ch.rect.left)
}

/// Whether `word` opens a list item: a lone sign that is no letter or digit
/// (a bullet, a dash, an asterisk), or a number or a single letter closed
/// by a full stop or a parenthesis (`1.`, `12)`, `a.`). A first word that
/// only looks like one, such as an initial, is taken for one too: the
/// mistake leaves lines apart.
fn is_list_marker(word: &str) -> bool {
    let mut chars = word.chars();
    let (Some(first), None) = (chars.next(), chars.clone().next()) else {
        let Some(body) = word.strip_suffix(['.', ')']) else {
            return false;
        };
        let mut letters = body.chars();
        return match (letters.next(), letters.next()) {
            (Some(letter), None) => letter.is_alphabetic() || letter.is_ascii_digit(),
            (Some(_), Some(_)) => body.chars().all(|c| c.is_ascii_digit()),
            (None, _) => false,
        };
    };
    !first.is_alphanumeric()
}
