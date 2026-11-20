// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Searching the text of a page. The text is compared as Unicode
//! compatibility-normalised text (UAX #15, form NFKC), so that a ligature
//! character (`U+FB01`) matches the two letters it stands for and a
//! full-width letter matches its plain form. Runs of white space, the
//! line and block breaks among them, count as one space, and a soft
//! hyphen (`U+00AD`) is not text. A match is a run of the page's
//! characters in reading order and may cross lines.

use unicode_normalization::UnicodeNormalization;

use crate::display::Rect;
use crate::text::TextPage;

/// What a search matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SearchOptions {
    /// Letters of different case do not match when this is set.
    pub case_sensitive: bool,
    /// A match is not part of a longer word: no letter or digit stands
    /// directly before or after it.
    pub whole_word: bool,
}

/// One match on a page.
#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    /// The first character of the match, an index into the page's
    /// characters in reading order ([`TextPage::char`]).
    pub start: usize,
    /// One past its last character.
    pub end: usize,
    /// The part of each line the match covers.
    pub rects: Vec<Rect>,
}

/// The text of a page as it is searched, with where each character came
/// from.
struct Haystack {
    chars: Vec<char>,
    /// For each character, the page character it comes from.
    origin: Vec<usize>,
}

const SOFT_HYPHEN: char = '\u{ad}';

/// Gives `push` the characters of `text` normalised, and folded to lower
/// case unless the search is case sensitive. A soft hyphen is not text.
fn fold(text: &str, case_sensitive: bool, mut push: impl FnMut(char)) {
    for ch in text.nfkc() {
        if ch == SOFT_HYPHEN {
            continue;
        }
        if case_sensitive || ch.is_whitespace() {
            push(ch);
        } else {
            ch.to_lowercase().for_each(&mut push);
        }
    }
}

impl TextPage {
    /// The text of the page in the form it is searched.
    fn haystack(&self, case_sensitive: bool) -> Haystack {
        let mut chars: Vec<char> = Vec::new();
        let mut origin: Vec<usize> = Vec::new();
        let mut spaced = true;
        let mut add = |ch: char, at: usize, chars: &mut Vec<char>, origin: &mut Vec<usize>| {
            if ch.is_whitespace() {
                if !spaced {
                    chars.push(' ');
                    origin.push(at);
                    spaced = true;
                }
            } else {
                chars.push(ch);
                origin.push(at);
                spaced = false;
            }
        };
        let mut last: Option<(usize, usize)> = None;
        for (at, place) in self.positions().iter().enumerate() {
            if last.is_some_and(|line| line != (place.block, place.line)) {
                add(' ', at.saturating_sub(1), &mut chars, &mut origin);
            }
            last = Some((place.block, place.line));
            let Some(ch) = self.char(at) else {
                continue;
            };
            fold(&ch.text, case_sensitive, |folded| {
                add(folded, at, &mut chars, &mut origin)
            });
        }
        Haystack { chars, origin }
    }

    /// The matches of `query`, in reading order, none overlapping the
    /// one before it. A query that is empty or only white space matches
    /// nothing.
    pub fn search(&self, query: &str, options: &SearchOptions) -> Vec<Match> {
        let mut needle: Vec<char> = Vec::new();
        let mut spaced = true;
        fold(query, options.case_sensitive, |ch| {
            if ch.is_whitespace() {
                if !spaced {
                    needle.push(' ');
                    spaced = true;
                }
            } else {
                needle.push(ch);
                spaced = false;
            }
        });
        if needle.last() == Some(&' ') {
            needle.pop();
        }
        if needle.is_empty() {
            return Vec::new();
        }
        let haystack = self.haystack(options.case_sensitive);
        let word = |ch: char| ch.is_alphanumeric();
        let mut found = Vec::new();
        let mut at = 0;
        while at + needle.len() <= haystack.chars.len() {
            let window = &haystack.chars[at..at + needle.len()];
            let whole = !options.whole_word
                || (at == 0 || !word(haystack.chars[at - 1]))
                    && haystack
                        .chars
                        .get(at + needle.len())
                        .is_none_or(|&after| !word(after));
            if window == needle.as_slice() && whole {
                let start = haystack.origin[at];
                let end = haystack.origin[at + needle.len() - 1] + 1;
                found.push(Match {
                    start,
                    end,
                    rects: self.rects_of(start, end),
                });
                at += needle.len();
            } else {
                at += 1;
            }
        }
        found
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::text::{TextBlock, TextChar, TextLine};

    /// A page of `lines`, one block, each character a `TextChar` of the
    /// given text.
    fn page_of(lines: &[&[&str]]) -> TextPage {
        let lines = lines
            .iter()
            .map(|chars| TextLine {
                rect: Rect::default(),
                chars: chars
                    .iter()
                    .map(|text| TextChar {
                        text: (*text).to_owned(),
                        rect: Rect::default(),
                        synthetic: false,
                        source: None,
                    })
                    .collect(),
            })
            .collect();
        TextPage::new(
            1,
            vec![TextBlock {
                rect: Rect::default(),
                lines,
            }],
            false,
        )
    }

    fn spans(page: &TextPage, query: &str) -> Vec<(usize, usize)> {
        page.search(query, &SearchOptions::default())
            .into_iter()
            .map(|found| (found.start, found.end))
            .collect()
    }

    #[test]
    fn a_soft_hyphen_is_not_text() {
        // `con` `\u{ad}` `cat`: the hyphen is a character of the page but
        // no part of what is searched.
        let page = page_of(&[&["c", "o", "n", "\u{ad}", "c", "a", "t"]]);
        assert_eq!(spans(&page, "concat"), [(0, 7)]);
    }

    #[test]
    fn compatibility_forms_match_their_plain_letters() {
        // A full-width `A` (U+FF21) and the ligature `ff` (U+FB00).
        let page = page_of(&[&["\u{FF21}", "\u{FB00}"]]);
        assert_eq!(spans(&page, "aff"), [(0, 2)]);
        assert_eq!(spans(&page, "a"), [(0, 1)]);
    }

    #[test]
    fn the_end_of_a_match_inside_a_ligature_takes_the_whole_character() {
        let page = page_of(&[&["\u{FB01}"]]);
        assert_eq!(spans(&page, "f"), [(0, 1)]);
    }

    #[test]
    fn a_line_break_is_one_space_and_leading_and_trailing_space_is_none() {
        let page = page_of(&[&["a", " ", " "], &[" ", "b"]]);
        assert_eq!(spans(&page, "a b"), [(0, 5)]);
        assert_eq!(spans(&page, " a b "), [(0, 5)]);
        assert!(spans(&page, "ab").is_empty());
    }

    #[test]
    fn a_lowercase_that_is_several_characters_credits_the_one_it_came_from() {
        // U+0130 lower-cases to `i` and a combining dot.
        let page = page_of(&[&["\u{130}"]]);
        assert_eq!(spans(&page, "i"), [(0, 1)]);
    }
}
