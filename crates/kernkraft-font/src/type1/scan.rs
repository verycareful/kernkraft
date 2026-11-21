// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! A scanner for the PostScript subset a Type 1 font program is written in:
//! names, numbers, strings, procedures and arrays, words, and comments
//! (Adobe Type 1 Font Format, the font program chapter; PostScript Language
//! Reference, the syntax chapter). It reads tokens only: nothing is run.

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Token<'a> {
    /// `/name`, without the slash.
    Name(&'a [u8]),
    /// An executable name: `def`, `dup`, `RD`, `-|` and the like.
    Word(&'a [u8]),
    Number(f64),
    /// `(text)` or `<hex>`, undecoded.
    Text(&'a [u8]),
    /// One of `{ } [ ]`.
    Bracket(u8),
}

pub(crate) struct Scanner<'a> {
    data: &'a [u8],
    pos: usize,
}

fn is_space(b: u8) -> bool {
    matches!(b, 0 | b'\t' | b'\n' | 0x0C | b'\r' | b' ')
}

fn is_delimiter(b: u8) -> bool {
    matches!(
        b,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

impl<'a> Scanner<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Scanner<'a> {
        Scanner { data, pos: 0 }
    }

    /// The next token, or `None` at the end of the data.
    pub(crate) fn next_token(&mut self) -> Option<Token<'a>> {
        loop {
            let b = *self.data.get(self.pos)?;
            if is_space(b) {
                self.pos += 1;
            } else if b == b'%' {
                while let Some(&c) = self.data.get(self.pos) {
                    if c == b'\n' || c == b'\r' {
                        break;
                    }
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
        let start = self.pos;
        let b = self.data[start];
        match b {
            b'/' => {
                self.pos += 1;
                let s = self.pos;
                self.run();
                Some(Token::Name(&self.data[s..self.pos]))
            }
            b'{' | b'}' | b'[' | b']' => {
                self.pos += 1;
                Some(Token::Bracket(b))
            }
            b'(' => Some(Token::Text(self.string())),
            b'<' | b'>' => {
                // `<<` and `>>` delimit a dictionary; a single `<...>` is a
                // hexadecimal string.
                if self.data.get(start + 1) == Some(&b) {
                    self.pos += 2;
                    return Some(Token::Word(&self.data[start..self.pos]));
                }
                if b == b'>' {
                    self.pos += 1;
                    return Some(Token::Word(&self.data[start..self.pos]));
                }
                let s = start + 1;
                let end = self.data[s..]
                    .iter()
                    .position(|&c| c == b'>')
                    .map_or(self.data.len(), |p| s + p);
                self.pos = (end + 1).min(self.data.len());
                Some(Token::Text(&self.data[s..end]))
            }
            b')' => {
                self.pos += 1;
                Some(Token::Word(&self.data[start..self.pos]))
            }
            _ => {
                self.run();
                let word = &self.data[start..self.pos];
                Some(match number(word) {
                    Some(n) => Token::Number(n),
                    None => Token::Word(word),
                })
            }
        }
    }

    /// Skips the characters of a name or word.
    fn run(&mut self) {
        while let Some(&c) = self.data.get(self.pos) {
            if is_space(c) || is_delimiter(c) {
                break;
            }
            self.pos += 1;
        }
    }

    /// A parenthesised string: balanced parentheses nest, a backslash
    /// protects the next byte.
    fn string(&mut self) -> &'a [u8] {
        let s = self.pos + 1;
        let mut depth = 0usize;
        let mut i = self.pos;
        while let Some(&c) = self.data.get(i) {
            match c {
                b'\\' => i += 1,
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        self.pos = i + 1;
                        return &self.data[s..i];
                    }
                }
                _ => {}
            }
            i += 1;
        }
        self.pos = self.data.len();
        &self.data[s..]
    }

    /// Reads the `len` bytes that follow the next single white-space byte,
    /// which is what a Type 1 `RD` operator consumes. `None` if they are
    /// not all there.
    pub(crate) fn binary(&mut self, len: usize) -> Option<&'a [u8]> {
        let start = self.pos.checked_add(1)?;
        let end = start.checked_add(len)?;
        let bytes = self.data.get(start..end)?;
        self.pos = end;
        Some(bytes)
    }
}

fn number(word: &[u8]) -> Option<f64> {
    let first = *word.first()?;
    if !(first.is_ascii_digit() || matches!(first, b'-' | b'+' | b'.')) {
        return None;
    }
    std::str::from_utf8(word)
        .ok()?
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn all(text: &[u8]) -> Vec<Token<'_>> {
        let mut s = Scanner::new(text);
        std::iter::from_fn(|| s.next_token()).collect()
    }

    #[test]
    fn names_words_numbers_and_comments() {
        use Token::*;
        assert_eq!(
            all(b"/FontName /Foo def % comment\n -12 .5 {1} [2]"),
            vec![
                Name(b"FontName"),
                Name(b"Foo"),
                Word(b"def"),
                Number(-12.0),
                Number(0.5),
                Bracket(b'{'),
                Number(1.0),
                Bracket(b'}'),
                Bracket(b'['),
                Number(2.0),
                Bracket(b']'),
            ]
        );
    }

    #[test]
    fn strings_nest_and_escape() {
        assert_eq!(
            all(b"(a(b)c\\)d) x"),
            vec![Token::Text(b"a(b)c\\)d"), Token::Word(b"x")]
        );
    }

    #[test]
    fn binary_follows_one_white_space_byte() {
        let mut s = Scanner::new(b"3 RD \x00\x01\x02 ND");
        assert_eq!(s.next_token(), Some(Token::Number(3.0)));
        assert_eq!(s.next_token(), Some(Token::Word(b"RD")));
        assert_eq!(s.binary(3), Some(&[0u8, 1, 2][..]));
        assert_eq!(s.next_token(), Some(Token::Word(b"ND")));
    }

    #[test]
    fn binary_past_the_end_is_none() {
        let mut s = Scanner::new(b"RD \x00");
        s.next_token();
        assert_eq!(s.binary(5), None);
        assert_eq!(s.binary(usize::MAX), None);
    }

    #[test]
    fn hyphen_bar_is_a_word_and_a_lone_hyphen_is_not_a_number() {
        assert_eq!(
            all(b"-| - ."),
            vec![Token::Word(b"-|"), Token::Word(b"-"), Token::Word(b".")]
        );
    }
}
