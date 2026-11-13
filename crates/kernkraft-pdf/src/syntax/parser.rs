// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Objects from tokens: ISO 32000-1:2008, 7.3.2 (booleans), 7.3.6 (arrays),
//! 7.3.7 (dictionaries), 7.3.9 (null), 7.3.10 (indirect objects and
//! references) and the `stream` keyword of 7.3.8.1.

use crate::diagnostic::{DiagnosticKind, Diagnostics, PortableLimit};
use crate::error::Limit;
use crate::syntax::lexer::unexpected;
use crate::syntax::limits::{MAX_NESTING_DEPTH, PORTABLE_OBJECT_COUNT};
use crate::syntax::{Dictionary, Lexer, Object, ObjectId, Span, Token, TokenKind, Value};
use crate::{PdfError, Source};

/// The nesting depth of an object that no array or dictionary contains.
const TOP_LEVEL_DEPTH: usize = 1;

/// An indirect object as far as this layer reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct IndirectObject {
    pub id: ObjectId,
    /// The object's value; for a stream, its dictionary.
    pub object: Object,
    /// The offset of the first byte of stream data, when the value is
    /// followed by the `stream` keyword. Where the data ends is for the
    /// caller to find from the dictionary's `Length`.
    pub stream_data_start: Option<u64>,
    /// From the object number to the end of `endobj`; for a stream, to
    /// `stream_data_start`; to the end of the value when `endobj` is
    /// missing.
    pub span: Span,
}

/// Reads objects from a [`Source`].
pub struct Parser<'s> {
    lexer: Lexer<'s>,
}

impl<'s> Parser<'s> {
    /// A parser at the start of `source`.
    pub fn new(source: &'s dyn Source) -> Self {
        Self {
            lexer: Lexer::new(source),
        }
    }

    /// The offset the next object is looked for from.
    pub fn position(&self) -> u64 {
        self.lexer.position()
    }

    /// Moves to `offset`; see [`Lexer::seek`].
    pub fn seek(&mut self, offset: u64) -> Result<(), PdfError> {
        self.lexer.seek(offset)
    }

    /// The lexer, for reading tokens and raw bytes between objects.
    pub fn lexer(&mut self) -> &mut Lexer<'s> {
        &mut self.lexer
    }

    /// One direct object or indirect reference, starting at the position.
    /// The parser is left straight after it.
    pub fn parse_object(&mut self, diagnostics: &mut Diagnostics) -> Result<Object, PdfError> {
        self.object(diagnostics, TOP_LEVEL_DEPTH, None)
    }

    /// One indirect object (7.3.10): its object number, its generation
    /// number and `obj`, its value, then `endobj` or `stream`. Diagnostics
    /// raised while reading it name it.
    ///
    /// After a value that `stream` follows, the parser is left at the first
    /// byte of the stream's data.
    pub fn parse_indirect_object(
        &mut self,
        diagnostics: &mut Diagnostics,
    ) -> Result<IndirectObject, PdfError> {
        let mark = diagnostics.len();
        let number = self.lexer.next_token(diagnostics)?;
        let start = number.span.start;
        let generation = self.lexer.next_token(diagnostics)?;
        let keyword = self.lexer.next_token(diagnostics)?;
        let id = ObjectId {
            number: header_part(&number, start)?,
            generation: header_part(&generation, start)?,
        };
        match keyword.kind {
            TokenKind::Keyword(ref bytes) if bytes == b"obj" => {}
            TokenKind::End => return Err(PdfError::UnexpectedEnd { offset: start }),
            ref other => return Err(unexpected(keyword.span.start, &token_text(other))),
        }
        // What the lexer said of the two numbers as plain integers does not
        // apply to an object identifier.
        diagnostics.truncate(mark);

        diagnostics.set_object(Some(id));
        let result = self.indirect_body(id, start, diagnostics);
        diagnostics.set_object(None);
        result
    }

    fn indirect_body(
        &mut self,
        id: ObjectId,
        start: u64,
        diagnostics: &mut Diagnostics,
    ) -> Result<IndirectObject, PdfError> {
        if id.number > PORTABLE_OBJECT_COUNT {
            diagnostics.push(
                DiagnosticKind::BeyondPortableLimit(PortableLimit::ObjectNumber),
                start,
            );
        }
        let object = self.object(diagnostics, TOP_LEVEL_DEPTH, Some(start))?;
        let value_end = self.lexer.position();

        let (stream_data_start, end) = match self.read_ahead()? {
            Some(Token {
                kind: TokenKind::Keyword(keyword),
                span,
            }) if keyword == b"endobj" => (None, span.end),
            Some(Token {
                kind: TokenKind::Keyword(keyword),
                span,
            }) if keyword == b"stream" => {
                if !matches!(object.value(), Value::Dictionary(_)) {
                    return Err(unexpected(span.start, &keyword));
                }
                let data_start = self.stream_data_start(span.start, diagnostics)?;
                (Some(data_start), data_start)
            }
            _ => {
                self.lexer.seek(value_end)?;
                diagnostics.push(DiagnosticKind::MissingEndobj, value_end);
                (None, value_end)
            }
        };
        Ok(IndirectObject {
            id,
            object,
            stream_data_start,
            span: Span { start, end },
        })
    }

    /// 7.3.8.1: the `stream` keyword is followed by a carriage return and a
    /// line feed, or by a line feed alone, and the data starts after them.
    /// Called with the lexer straight after the keyword.
    fn stream_data_start(
        &mut self,
        keyword_offset: u64,
        diagnostics: &mut Diagnostics,
    ) -> Result<u64, PdfError> {
        let cursor = self.lexer.cursor();
        match (cursor.peek()?, cursor.peek_ahead(1)?) {
            (Some(b'\r'), Some(b'\n')) => cursor.advance(2),
            (Some(b'\n'), _) => cursor.advance(1),
            (Some(b'\r'), _) => {
                cursor.advance(1);
                diagnostics.push(DiagnosticKind::StreamKeywordEol, keyword_offset);
            }
            _ => diagnostics.push(DiagnosticKind::StreamKeywordEol, keyword_offset),
        }
        Ok(cursor.position())
    }

    /// The object that starts with the next token. `enclosing` is where the
    /// innermost construct still open starts, for the error when the source
    /// ends first.
    fn object(
        &mut self,
        diagnostics: &mut Diagnostics,
        depth: usize,
        enclosing: Option<u64>,
    ) -> Result<Object, PdfError> {
        let mark = diagnostics.len();
        let token = self.lexer.next_token(diagnostics)?;
        self.object_from(token, mark, diagnostics, depth, enclosing)
    }

    /// The object that starts with `first`. `mark` is how many diagnostics
    /// there were before `first` was read.
    fn object_from(
        &mut self,
        first: Token,
        mark: usize,
        diagnostics: &mut Diagnostics,
        depth: usize,
        enclosing: Option<u64>,
    ) -> Result<Object, PdfError> {
        let span = first.span;
        let value = match first.kind {
            TokenKind::End => {
                return Err(PdfError::UnexpectedEnd {
                    offset: enclosing.unwrap_or(span.start),
                })
            }
            TokenKind::Integer(integer) => {
                return self.integer_or_reference(integer, span, mark, diagnostics)
            }
            TokenKind::Real(real) => Value::Real(real),
            TokenKind::String(string) => Value::String(string),
            TokenKind::Name(name) => Value::Name(name),
            TokenKind::ArrayOpen => return self.array(span.start, diagnostics, depth),
            TokenKind::DictOpen => return self.dictionary(span.start, diagnostics, depth),
            TokenKind::Keyword(ref keyword) => match keyword.as_slice() {
                b"true" => Value::Boolean(true),
                b"false" => Value::Boolean(false),
                b"null" => Value::Null,
                other => return Err(unexpected(span.start, other)),
            },
            ref closer @ (TokenKind::ArrayClose | TokenKind::DictClose) => {
                return Err(unexpected(span.start, &token_text(closer)))
            }
        };
        Ok(Object::parsed(value, span))
    }

    /// An integer, or the indirect reference it starts: a reference is two
    /// integers that are not negative and the keyword `R` (7.3.10).
    fn integer_or_reference(
        &mut self,
        integer: i64,
        span: Span,
        mark: usize,
        diagnostics: &mut Diagnostics,
    ) -> Result<Object, PdfError> {
        let after_integer = self.lexer.position();
        if integer >= 0 {
            if let Some(end) = self.rest_of_reference()? {
                // The lexer judged the number as a plain integer; as an
                // object number it is judged below.
                diagnostics.truncate(mark);
                let span = Span {
                    start: span.start,
                    end: end.offset,
                };
                let id = u32::try_from(integer)
                    .ok()
                    .zip(u16::try_from(end.generation).ok())
                    .map(|(number, generation)| ObjectId { number, generation });
                let value = match id {
                    Some(id) => {
                        if id.number > PORTABLE_OBJECT_COUNT {
                            diagnostics.push(
                                DiagnosticKind::BeyondPortableLimit(PortableLimit::ObjectNumber),
                                span.start,
                            );
                        }
                        Value::Reference(id)
                    }
                    // No object can carry such an identifier, and a
                    // reference to an object that does not exist is the
                    // null object (7.3.10).
                    None => {
                        diagnostics.push(DiagnosticKind::ReferenceOutOfRange, span.start);
                        Value::Null
                    }
                };
                return Ok(Object::parsed(value, span));
            }
            self.lexer.seek(after_integer)?;
        }
        Ok(Object::parsed(Value::Integer(integer), span))
    }

    /// Reads ahead for the generation number and `R` that would make the
    /// integer just read a reference. When they are not there the lexer is
    /// left wherever the look stopped and the caller moves it back.
    fn rest_of_reference(&mut self) -> Result<Option<ReferenceEnd>, PdfError> {
        let generation = match self.read_ahead()? {
            Some(Token {
                kind: TokenKind::Integer(generation),
                ..
            }) if generation >= 0 => generation,
            _ => return Ok(None),
        };
        Ok(match self.read_ahead()? {
            Some(Token {
                kind: TokenKind::Keyword(keyword),
                span,
            }) if keyword == b"R" => Some(ReferenceEnd {
                generation,
                offset: span.end,
            }),
            _ => None,
        })
    }

    /// The next token, read to decide what the tokens before it are.
    /// Diagnostics it raises are dropped: when it is not consumed it is
    /// read again by whoever does consume it. Bytes that are not a token
    /// give `None` for the same reason, so that their error is reported
    /// against the read that needs them.
    fn read_ahead(&mut self) -> Result<Option<Token>, PdfError> {
        match self.lexer.next_token(&mut Diagnostics::new()) {
            Ok(token) => Ok(Some(token)),
            Err(
                PdfError::UnexpectedEnd { .. }
                | PdfError::UnexpectedToken { .. }
                | PdfError::LimitExceeded { .. },
            ) => Ok(None),
            Err(other) => Err(other),
        }
    }

    /// 7.3.6: the objects up to the closing bracket. `open` is the offset
    /// of the opening one, which has been read.
    fn array(
        &mut self,
        open: u64,
        diagnostics: &mut Diagnostics,
        depth: usize,
    ) -> Result<Object, PdfError> {
        check_depth(depth, open)?;
        let mut items = Vec::new();
        loop {
            let mark = diagnostics.len();
            let token = self.lexer.next_token(diagnostics)?;
            if token.kind == TokenKind::ArrayClose {
                let span = Span {
                    start: open,
                    end: token.span.end,
                };
                return Ok(Object::parsed(Value::Array(items), span));
            }
            items.push(self.object_from(token, mark, diagnostics, depth + 1, Some(open))?);
        }
    }

    /// 7.3.7: pairs of a name and an object up to the closing brackets.
    /// `open` is the offset of the opening ones, which have been read.
    fn dictionary(
        &mut self,
        open: u64,
        diagnostics: &mut Diagnostics,
        depth: usize,
    ) -> Result<Object, PdfError> {
        check_depth(depth, open)?;
        let mut dictionary = Dictionary::new();
        loop {
            let token = self.lexer.next_token(diagnostics)?;
            let key = match token.kind {
                TokenKind::DictClose => {
                    let span = Span {
                        start: open,
                        end: token.span.end,
                    };
                    return Ok(Object::parsed(Value::Dictionary(dictionary), span));
                }
                TokenKind::Name(key) => key,
                TokenKind::End => return Err(PdfError::UnexpectedEnd { offset: open }),
                ref other => return Err(unexpected(token.span.start, &token_text(other))),
            };
            let value = self.object(diagnostics, depth + 1, Some(open))?;
            if dictionary.insert(key.clone(), value).is_some() {
                diagnostics.push(DiagnosticKind::DuplicateKey { key }, token.span.start);
            }
        }
    }
}

/// The generation number of a reference and the offset after its `R`.
struct ReferenceEnd {
    generation: i64,
    offset: u64,
}

/// Refuses a container that would nest deeper than the limit. `open` is
/// the offset of its opening delimiter.
fn check_depth(depth: usize, open: u64) -> Result<(), PdfError> {
    if depth > MAX_NESTING_DEPTH {
        return Err(PdfError::LimitExceeded {
            offset: open,
            limit: Limit::NestingDepth,
        });
    }
    Ok(())
}

/// The object number or generation number of an `obj` header: an integer
/// that is not negative and fits the identifier's field. `start` is where
/// the header starts, for the error when the source ends inside it.
fn header_part<T: TryFrom<i64>>(token: &Token, start: u64) -> Result<T, PdfError> {
    match token.kind {
        TokenKind::End => Err(PdfError::UnexpectedEnd { offset: start }),
        TokenKind::Integer(value) => {
            T::try_from(value).map_err(|_| unexpected(token.span.start, &token_text(&token.kind)))
        }
        ref other => Err(unexpected(token.span.start, &token_text(other))),
    }
}

/// A token as text, for the error that names it. Values are shown in the
/// canonical notation, not as they were written.
fn token_text(kind: &TokenKind) -> Vec<u8> {
    match kind {
        TokenKind::Integer(value) => value.to_string().into_bytes(),
        TokenKind::Real(value) => value.get().to_string().into_bytes(),
        TokenKind::String(string) => [b"(", string.as_bytes(), b")"].concat(),
        TokenKind::Name(name) => [b"/", name.as_bytes()].concat(),
        TokenKind::ArrayOpen => b"[".to_vec(),
        TokenKind::ArrayClose => b"]".to_vec(),
        TokenKind::DictOpen => b"<<".to_vec(),
        TokenKind::DictClose => b">>".to_vec(),
        TokenKind::Keyword(keyword) => keyword.clone(),
        TokenKind::End => b"the end of the file".to_vec(),
    }
}
