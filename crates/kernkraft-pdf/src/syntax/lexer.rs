// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Tokens: ISO 32000-1:2008, 7.2.2 (character set), 7.2.3 (comments), 7.3.3
//! (numbers), 7.3.4.2 (literal strings), 7.3.4.3 (hexadecimal strings) and
//! 7.3.5 (names).

use crate::diagnostic::{DiagnosticKind, Diagnostics, PortableLimit};
use crate::error::Limit;
use crate::syntax::chars::{hex_value, is_regular, is_whitespace};
use crate::syntax::limits::{
    MAX_NAME_BYTES, MAX_STRING_BYTES, MAX_TOKEN_BYTES, PORTABLE_INTEGER_MAX, PORTABLE_INTEGER_MIN,
    PORTABLE_NAME_BYTES, PORTABLE_REAL_MAX,
};
use crate::syntax::{Cursor, Name, PdfString, Real, Span};
use crate::{PdfError, Source};

/// How many bytes of a token an [`PdfError::UnexpectedToken`] quotes: enough
/// to recognise the token, short enough for one line of a message.
const FOUND_BYTES: usize = 32;

/// The octal digits a `\ddd` escape holds at most (7.3.4.2).
const OCTAL_ESCAPE_DIGITS: usize = 3;

/// What a token is, with its value where it has one.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Integer(i64),
    Real(Real),
    /// A literal or hexadecimal string, decoded.
    String(PdfString),
    Name(Name),
    /// `[`
    ArrayOpen,
    /// `]`
    ArrayClose,
    /// `<<`
    DictOpen,
    /// `>>`
    DictClose,
    /// A run of regular characters that is not a number (`true`, `obj`, `R`,
    /// a content-stream operator), or a single `{` or `}`.
    Keyword(Vec<u8>),
    /// The end of the source.
    End,
}

/// A token and the bytes it was read from, delimiters included.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

/// The error for a token that cannot stand where it was found.
pub(crate) fn unexpected(offset: u64, token: &[u8]) -> PdfError {
    let shown = token.get(..FOUND_BYTES).unwrap_or(token);
    PdfError::UnexpectedToken {
        offset,
        found: String::from_utf8_lossy(shown).into_owned(),
    }
}

/// Reads tokens from a [`Source`].
pub struct Lexer<'s> {
    cursor: Cursor<'s>,
}

impl<'s> Lexer<'s> {
    /// A lexer at the start of `source`.
    pub fn new(source: &'s dyn Source) -> Self {
        Self {
            cursor: Cursor::new(source),
        }
    }

    /// The offset the next token is looked for from.
    pub fn position(&self) -> u64 {
        self.cursor.position()
    }

    /// Moves to `offset`; see [`Cursor::seek`].
    pub fn seek(&mut self, offset: u64) -> Result<(), PdfError> {
        self.cursor.seek(offset)
    }

    /// The cursor, for bytes that are not tokens: stream data and the data
    /// of an inline image.
    pub fn cursor(&mut self) -> &mut Cursor<'s> {
        &mut self.cursor
    }

    /// The next token, after any white space and comments. At the end of
    /// the source it is [`TokenKind::End`], however often it is asked for.
    pub fn next_token(&mut self, diagnostics: &mut Diagnostics) -> Result<Token, PdfError> {
        self.skip_whitespace_and_comments()?;
        let start = self.cursor.position();
        let kind = match self.cursor.peek()? {
            None => TokenKind::End,
            Some(b'[') => self.single(TokenKind::ArrayOpen),
            Some(b']') => self.single(TokenKind::ArrayClose),
            Some(byte @ (b'{' | b'}')) => self.single(TokenKind::Keyword(vec![byte])),
            Some(b'<') => {
                if self.cursor.peek_ahead(1)? == Some(b'<') {
                    self.double(TokenKind::DictOpen)
                } else {
                    self.hex_string(start, diagnostics)?
                }
            }
            Some(b'>') => {
                if self.cursor.peek_ahead(1)? == Some(b'>') {
                    self.double(TokenKind::DictClose)
                } else {
                    return Err(unexpected(start, b">"));
                }
            }
            Some(b'(') => self.literal_string(start)?,
            Some(b')') => return Err(unexpected(start, b")")),
            Some(b'/') => self.name(start, diagnostics)?,
            Some(_) => self.number_or_keyword(start, diagnostics)?,
        };
        Ok(Token {
            kind,
            span: Span {
                start,
                end: self.cursor.position(),
            },
        })
    }

    /// Moves past the one-byte token at the position.
    fn single(&mut self, kind: TokenKind) -> TokenKind {
        self.advance(1);
        kind
    }

    /// Moves past the two-byte token at the position.
    fn double(&mut self, kind: TokenKind) -> TokenKind {
        self.advance(2);
        kind
    }

    /// Moves past `count` bytes that a peek has shown to exist.
    fn advance(&mut self, count: u64) {
        self.cursor.advance(count);
    }

    /// 7.2.2 and 7.2.3: white space separates tokens, and a comment runs
    /// from a percent sign to the end of its line and counts as white space.
    fn skip_whitespace_and_comments(&mut self) -> Result<(), PdfError> {
        while let Some(byte) = self.cursor.peek()? {
            if is_whitespace(byte) {
                self.advance(1);
            } else if byte == b'%' {
                while !matches!(self.cursor.peek()?, None | Some(b'\r' | b'\n')) {
                    self.advance(1);
                }
            } else {
                break;
            }
        }
        Ok(())
    }

    /// A run of regular characters: a number when it is written as 7.3.3
    /// says, otherwise a keyword.
    fn number_or_keyword(
        &mut self,
        start: u64,
        diagnostics: &mut Diagnostics,
    ) -> Result<TokenKind, PdfError> {
        let mut run = Vec::new();
        while let Some(byte) = self.cursor.peek()? {
            if !is_regular(byte) {
                break;
            }
            if run.len() == MAX_TOKEN_BYTES {
                return Err(PdfError::LimitExceeded {
                    offset: start,
                    limit: Limit::TokenBytes,
                });
            }
            run.push(byte);
            self.advance(1);
        }
        Ok(match number(&run) {
            Some(Number::Integer(value)) => {
                if !(PORTABLE_INTEGER_MIN..=PORTABLE_INTEGER_MAX).contains(&value) {
                    diagnostics.push(
                        DiagnosticKind::BeyondPortableLimit(PortableLimit::Integer),
                        start,
                    );
                }
                TokenKind::Integer(value)
            }
            Some(Number::TooLargeInteger(value)) => {
                diagnostics.push(DiagnosticKind::IntegerOverflow, start);
                TokenKind::Real(finite(value, start, diagnostics))
            }
            Some(Number::Real(value)) => {
                if value.is_finite() && value.abs() > PORTABLE_REAL_MAX {
                    diagnostics.push(
                        DiagnosticKind::BeyondPortableLimit(PortableLimit::Real),
                        start,
                    );
                }
                TokenKind::Real(finite(value, start, diagnostics))
            }
            None => TokenKind::Keyword(run),
        })
    }

    /// 7.3.5: a solidus, then regular characters, any of which may be
    /// written as a number sign and two hexadecimal digits.
    fn name(&mut self, start: u64, diagnostics: &mut Diagnostics) -> Result<TokenKind, PdfError> {
        self.advance(1);
        let mut bytes = Vec::new();
        while let Some(byte) = self.cursor.peek()? {
            if !is_regular(byte) {
                break;
            }
            let escape_offset = self.cursor.position();
            let decoded = if byte == b'#' {
                let high = self.cursor.peek_ahead(1)?.and_then(hex_value);
                let low = self.cursor.peek_ahead(2)?.and_then(hex_value);
                match (high, low) {
                    (Some(high), Some(low)) => {
                        self.advance(3);
                        let value = high << 4 | low;
                        if value == 0 {
                            diagnostics.push(DiagnosticKind::NulInName, escape_offset);
                        }
                        value
                    }
                    _ => {
                        diagnostics.push(DiagnosticKind::InvalidNameEscape, escape_offset);
                        self.advance(1);
                        byte
                    }
                }
            } else {
                self.advance(1);
                byte
            };
            if bytes.len() == MAX_NAME_BYTES {
                return Err(PdfError::LimitExceeded {
                    offset: start,
                    limit: Limit::NameBytes,
                });
            }
            bytes.push(decoded);
        }
        if bytes.len() > PORTABLE_NAME_BYTES {
            diagnostics.push(
                DiagnosticKind::BeyondPortableLimit(PortableLimit::NameLength),
                start,
            );
        }
        Ok(TokenKind::Name(Name::new(bytes)))
    }

    /// 7.3.4.2: bytes between balanced parentheses, with the escapes of
    /// Table 3.
    fn literal_string(&mut self, start: u64) -> Result<TokenKind, PdfError> {
        self.advance(1);
        let mut bytes = StringBytes::new(start);
        // The opening parenthesis is the first level; the string ends when
        // its match closes it.
        let mut open_parentheses: u64 = 1;
        loop {
            match self.string_byte(start)? {
                b'(' => {
                    open_parentheses += 1;
                    bytes.push(b'(')?;
                }
                b')' => {
                    open_parentheses -= 1;
                    if open_parentheses == 0 {
                        break;
                    }
                    bytes.push(b')')?;
                }
                b'\\' => {
                    if let Some(byte) = self.escape(start)? {
                        bytes.push(byte)?;
                    }
                }
                b'\r' => {
                    self.skip_line_feed()?;
                    bytes.push(b'\n')?;
                }
                byte => bytes.push(byte)?,
            }
        }
        Ok(TokenKind::String(PdfString::new(bytes.finish())))
    }

    /// The byte a backslash escape stands for, or `None` for a backslash
    /// that ends a line, which stands for nothing.
    fn escape(&mut self, start: u64) -> Result<Option<u8>, PdfError> {
        Ok(Some(match self.string_byte(start)? {
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'b' => 0x08,
            b'f' => 0x0C,
            first @ b'0'..=b'7' => {
                let mut code = u32::from(first - b'0');
                for _ in 1..OCTAL_ESCAPE_DIGITS {
                    match self.cursor.peek()? {
                        Some(digit @ b'0'..=b'7') => {
                            code = code * 8 + u32::from(digit - b'0');
                            self.advance(1);
                        }
                        _ => break,
                    }
                }
                // "High-order overflow shall be ignored."
                (code & 0xFF) as u8
            }
            b'\r' => {
                self.skip_line_feed()?;
                return Ok(None);
            }
            b'\n' => return Ok(None),
            // Table 3's parentheses and backslash stand for themselves, and
            // before any other byte the backslash is ignored.
            other => other,
        }))
    }

    /// The next byte of a string that starts at `start`, which must exist.
    fn string_byte(&mut self, start: u64) -> Result<u8, PdfError> {
        self.cursor
            .next()?
            .ok_or(PdfError::UnexpectedEnd { offset: start })
    }

    /// After a carriage return: a line feed that follows belongs to the
    /// same end-of-line marker.
    fn skip_line_feed(&mut self) -> Result<(), PdfError> {
        if self.cursor.peek()? == Some(b'\n') {
            self.advance(1);
        }
        Ok(())
    }

    /// 7.3.4.3: pairs of hexadecimal digits between angle brackets, white
    /// space ignored, a missing final digit taken as 0.
    fn hex_string(
        &mut self,
        start: u64,
        diagnostics: &mut Diagnostics,
    ) -> Result<TokenKind, PdfError> {
        self.advance(1);
        let mut bytes = StringBytes::new(start);
        let mut high: Option<u8> = None;
        loop {
            let offset = self.cursor.position();
            let byte = self.string_byte(start)?;
            if byte == b'>' {
                break;
            }
            match (hex_value(byte), high) {
                (Some(digit), None) => high = Some(digit),
                (Some(digit), Some(first)) => {
                    bytes.push(first << 4 | digit)?;
                    high = None;
                }
                (None, _) if is_whitespace(byte) => {}
                (None, _) => diagnostics.push(DiagnosticKind::InvalidHexDigit { byte }, offset),
            }
        }
        if let Some(first) = high {
            bytes.push(first << 4)?;
        }
        Ok(TokenKind::String(PdfString::new(bytes.finish())))
    }
}

/// The decoded bytes of a string being read, held to [`MAX_STRING_BYTES`].
struct StringBytes {
    bytes: Vec<u8>,
    start: u64,
}

impl StringBytes {
    fn new(start: u64) -> Self {
        Self {
            bytes: Vec::new(),
            start,
        }
    }

    fn push(&mut self, byte: u8) -> Result<(), PdfError> {
        if self.bytes.len() == MAX_STRING_BYTES {
            return Err(PdfError::LimitExceeded {
                offset: self.start,
                limit: Limit::StringBytes,
            });
        }
        self.bytes.push(byte);
        Ok(())
    }

    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

/// What a run of regular characters is when it is a number.
enum Number {
    Integer(i64),
    /// Written as an integer, too large for an `i64`.
    TooLargeInteger(f64),
    Real(f64),
}

/// 7.3.3: an optional sign, then decimal digits with at most one period
/// among, before or after them, and at least one digit. No exponent and no
/// radix.
fn number(run: &[u8]) -> Option<Number> {
    let unsigned = match run {
        [b'+' | b'-', rest @ ..] => rest,
        all => all,
    };
    let periods = unsigned.iter().filter(|&&byte| byte == b'.').count();
    let digits = unsigned.iter().filter(|byte| byte.is_ascii_digit()).count();
    if digits == 0 || periods > 1 || digits + periods != unsigned.len() {
        return None;
    }
    // Signs, digits and periods are ASCII, and Rust reads this notation as
    // 7.3.3 defines it, a leading or trailing period included.
    let text = std::str::from_utf8(run).ok()?;
    if periods == 1 {
        return text.parse().ok().map(Number::Real);
    }
    match text.parse() {
        Ok(value) => Some(Number::Integer(value)),
        Err(_) => text.parse().ok().map(Number::TooLargeInteger),
    }
}

/// `value` as a real, clamped to the largest finite value of its sign when
/// the digits written have no finite `f64`.
fn finite(value: f64, offset: u64, diagnostics: &mut Diagnostics) -> Real {
    if !value.is_finite() {
        diagnostics.push(DiagnosticKind::RealOverflow, offset);
    }
    Real::saturating(value)
}
