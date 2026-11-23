// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Searching the text of a page. The text is compared as Unicode
//! compatibility-normalised text (UAX #15, form NFKC), so that a ligature
//! character (`U+FB01`) matches the two letters it stands for and a
//! full-width letter matches its plain form. Runs of white space, the
//! line and block breaks among them, count as one space, and a soft
//! hyphen (`U+00AD`) is not text. A match is a run of the page's
//! characters in reading order and may cross lines.
//!
//! A hyphen that ends a line between two letters may be where a word was
//! broken or part of the word. A match may span it and the line break
//! whichever it is: a query spells the word with the hyphen and no break
//! (`well-known`), without both (`wellknown`), or as the text reads (`well-
//! known`). The text of the page is not changed.

use std::cell::Cell;

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
    /// Whether a line-end hyphen was joined.
    joined: bool,
}

/// What a line end between two letters after a hyphen becomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Join {
    /// A space, as at any line end.
    Space,
    /// The hyphen stays and no space follows it.
    Kept,
    /// Neither the hyphen nor a space is text.
    Dropped,
}

/// The characters that end a line as a hyphen: the hyphen-minus and U+2010
/// (HYPHEN). A hyphen is also one of the dashes of Unicode's `Pd`, but a dash
/// at a line end is not a place where a word was broken.
const LINE_END_HYPHENS: [char; 2] = ['-', '\u{2010}'];

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
    /// Whether the line that starts at character `at` begins with a letter,
    /// white space before it left out.
    fn line_starts_with_letter(&self, at: usize) -> bool {
        let line = self.positions()[at];
        (at..self.positions().len())
            .take_while(|&index| {
                let place = self.positions()[index];
                (place.block, place.line) == (line.block, line.line)
            })
            .filter_map(|index| self.char(index)?.text.chars().next())
            .find(|ch| !ch.is_whitespace())
            .is_some_and(char::is_alphabetic)
    }

    /// The text of the page in the form it is searched.
    fn haystack(&self, case_sensitive: bool, join: Join) -> Haystack {
        let mut chars: Vec<char> = Vec::new();
        let mut origin: Vec<usize> = Vec::new();
        let spaced = Cell::new(true);
        let mut joined = false;
        let add = |ch: char, at: usize, chars: &mut Vec<char>, origin: &mut Vec<usize>| {
            if ch.is_whitespace() {
                if !spaced.get() {
                    chars.push(' ');
                    origin.push(at);
                    spaced.set(true);
                }
            } else {
                chars.push(ch);
                origin.push(at);
                spaced.set(false);
            }
        };
        let mut last: Option<(usize, usize)> = None;
        for (at, place) in self.positions().iter().enumerate() {
            if last.is_some_and(|line| line != (place.block, place.line)) {
                // The text so far, without the space white space at the end
                // of the line left.
                let end = chars.len() - usize::from(chars.last() == Some(&' '));
                let hyphenated = join != Join::Space
                    && end >= 2
                    && LINE_END_HYPHENS.contains(&chars[end - 1])
                    && chars[end - 2].is_alphabetic()
                    && self.line_starts_with_letter(at);
                if hyphenated {
                    let keep = if join == Join::Dropped { end - 1 } else { end };
                    chars.truncate(keep);
                    origin.truncate(keep);
                    spaced.set(false);
                    joined = true;
                } else {
                    add(' ', at.saturating_sub(1), &mut chars, &mut origin);
                }
            }
            last = Some((place.block, place.line));
            let Some(ch) = self.char(at) else {
                continue;
            };
            fold(&ch.text, case_sensitive, |folded| {
                add(folded, at, &mut chars, &mut origin)
            });
        }
        Haystack {
            chars,
            origin,
            joined,
        }
    }

    /// The matches of `needle` in `haystack`, in reading order, none
    /// overlapping the one before it.
    fn find(&self, haystack: &Haystack, needle: &[char], whole_word: bool) -> Vec<Match> {
        let word = |ch: char| ch.is_alphanumeric();
        let mut found = Vec::new();
        let mut at = 0;
        while at + needle.len() <= haystack.chars.len() {
            let window = &haystack.chars[at..at + needle.len()];
            let whole = !whole_word
                || (at == 0 || !word(haystack.chars[at - 1]))
                    && haystack
                        .chars
                        .get(at + needle.len())
                        .is_none_or(|&after| !word(after));
            if window == needle && whole {
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
        let mut found = self.find(
            &self.haystack(options.case_sensitive, Join::Space),
            &needle,
            options.whole_word,
        );
        // A page with a hyphen that ends a line between letters is read two
        // more ways: the hyphen kept and the break gone, and both gone.
        let kept = self.haystack(options.case_sensitive, Join::Kept);
        if kept.joined {
            found.extend(self.find(&kept, &needle, options.whole_word));
            let dropped = self.haystack(options.case_sensitive, Join::Dropped);
            found.extend(self.find(&dropped, &needle, options.whole_word));
            found.sort_by_key(|found| found.start);
            let mut end = 0;
            found.retain(|found| {
                let keep = found.start >= end;
                if keep {
                    end = found.end;
                }
                keep
            });
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
            .map(|chars| {
                TextLine::new(
                    Rect::default(),
                    chars
                        .iter()
                        .map(|text| TextChar {
                            text: (*text).to_owned(),
                            rect: Rect::default(),
                            quad: [(0.0, 0.0); 4],
                            synthetic: false,
                            source: None,
                        })
                        .collect(),
                )
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

    fn broken_word() -> TextPage {
        page_of(&[&["w", "e", "l", "l", "-"], &["k", "n", "o", "w", "n"]])
    }

    #[test]
    fn a_word_broken_at_a_line_end_hyphen_is_found_whole() {
        assert_eq!(spans(&broken_word(), "wellknown"), [(0, 10)]);
    }

    #[test]
    fn the_hyphen_may_be_part_of_the_query() {
        assert_eq!(spans(&broken_word(), "well-known"), [(0, 10)]);
        assert_eq!(spans(&broken_word(), "well- known"), [(0, 10)]);
    }

    #[test]
    fn a_part_of_a_broken_word_is_found_once() {
        assert_eq!(spans(&broken_word(), "known"), [(5, 10)]);
        assert_eq!(spans(&broken_word(), "well"), [(0, 4)]);
    }

    #[test]
    fn a_match_that_spans_the_break_gives_a_rectangle_for_each_line() {
        let page = broken_word();
        let found = page.search("wellknown", &SearchOptions::default());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].rects.len(), 2);
    }

    #[test]
    fn a_hyphen_is_joined_only_between_letters() {
        // A range of years, a hyphen after a space, and a hyphen at the very
        // end of the text have no word to join.
        let years = page_of(&[&["1", "9", "8", "0", "-"], &["2", "0", "0", "0"]]);
        assert!(spans(&years, "19802000").is_empty());
        let spaced = page_of(&[&["a", " ", "-"], &["b"]]);
        assert!(spans(&spaced, "ab").is_empty());
        let last = page_of(&[&["a", "-"]]);
        assert_eq!(spans(&last, "a-"), [(0, 2)]);
        let digit = page_of(&[&["a", "-"], &["1"]]);
        assert!(spans(&digit, "a1").is_empty());
    }

    #[test]
    fn a_page_with_no_line_end_hyphen_is_searched_as_before() {
        let page = page_of(&[&["a", "b"], &["c", "d"]]);
        assert_eq!(spans(&page, "b c"), [(1, 3)]);
        assert!(spans(&page, "bc").is_empty());
    }

    #[test]
    fn the_hyphen_of_a_unicode_hyphen_character_counts_too() {
        let page = page_of(&[
            &["w", "e", "l", "l", "\u{2010}"],
            &["k", "n", "o", "w", "n"],
        ]);
        assert_eq!(spans(&page, "wellknown"), [(0, 10)]);
    }

    #[test]
    fn a_lowercase_that_is_several_characters_credits_the_one_it_came_from() {
        // U+0130 lower-cases to `i` and a combining dot.
        let page = page_of(&[&["\u{130}"]]);
        assert_eq!(spans(&page, "i"), [(0, 1)]);
    }
}
