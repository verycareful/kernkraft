// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Reading a content stream as operations: ISO 32000-1:2008, 7.8.2 (content
//! streams), 8.2 (graphics objects) and 8.9.7 (inline images).
//!
//! An [`Operation`] is the operands that precede an operator, the operator,
//! and the byte span they were read from, so that an editor can cut the
//! operation out of the stream or rewrite it. An inline image (`BI`, its
//! dictionary, `ID`, its data, `EI`) is one operation.

use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::error::{Limit, PdfError};
use crate::source::Source;
use crate::syntax::chars::{is_delimiter, is_whitespace};
use crate::syntax::limits::MAX_NESTING_DEPTH;
use crate::syntax::{Dictionary, Lexer, Name, Object, Span, TokenKind, Value};

/// The most operands the operand stack holds before an operator. Table C.1
/// gives 8191 as the most elements of an array that every consumer
/// accepts, and an operator's operands (the `TJ` items are one array) are
/// far fewer in any real stream; a stream that pushes more has its
/// earliest operands dropped.
pub const MAX_OPERANDS: usize = 8191;

/// The bytes after `EI` that are looked at to tell the keyword from image
/// data that happens to hold the letters `EI` (8.9.7 says only that `EI`
/// follows the data).
const EI_LOOKAHEAD: usize = 15;

/// How many bits a byte holds.
const BITS_PER_BYTE: u64 = 8;

/// One operation of a content stream.
#[derive(Debug, Clone, PartialEq)]
pub struct Operation {
    /// The operator's bytes: `Tj`, `cm`, `BI`.
    pub operator: Vec<u8>,
    /// The operands, in the order written. Each remembers its own span.
    pub operands: Vec<Object>,
    /// From the first operand (or the operator, with no operands) to the
    /// end of the operator; for an inline image, to the end of `EI`.
    pub span: Span,
    /// The dictionary and data of an inline image, for `BI` only.
    pub inline_image: Option<InlineImage>,
}

/// An inline image (8.9.7).
#[derive(Debug, Clone, PartialEq)]
pub struct InlineImage {
    /// The image dictionary with the abbreviated keys of Table 93 and the
    /// abbreviated colour space and filter names of Tables 94 and 6
    /// written out.
    pub dictionary: Dictionary,
    /// The bytes of the data, as the filters encoded it, in the stream.
    pub data: Span,
}

/// Reads the operations of one content stream.
pub struct ContentParser<'s> {
    source: &'s dyn Source,
    lexer: Lexer<'s>,
}

impl<'s> ContentParser<'s> {
    pub fn new(source: &'s dyn Source) -> Self {
        Self {
            source,
            lexer: Lexer::new(source),
        }
    }

    /// The offset the next operation is looked for from.
    pub fn position(&self) -> u64 {
        self.lexer.position()
    }

    /// The next operation, or `None` at the end of the stream. Operands
    /// left over at the end, with no operator after them, are dropped with
    /// the diagnostic [`DiagnosticKind::OperandsInvalid`] naming no
    /// operator.
    pub fn next_operation(
        &mut self,
        diagnostics: &mut Diagnostics,
    ) -> Result<Option<Operation>, PdfError> {
        let mut operands: Vec<Object> = Vec::new();
        let mut first: Option<u64> = None;
        loop {
            let token = match self.lexer.next_token(diagnostics) {
                Ok(token) => token,
                Err(PdfError::UnexpectedToken { offset, .. }) => {
                    // A stray `)` or `>`: skip the byte.
                    diagnostics.push(DiagnosticKind::ContentByteSkipped, offset);
                    self.lexer.seek(offset + 1)?;
                    continue;
                }
                Err(error) => return Err(error),
            };
            let start = token.span.start;
            let object = match token.kind {
                TokenKind::End => {
                    if !operands.is_empty() {
                        diagnostics.push(
                            DiagnosticKind::OperandsInvalid {
                                operator: Name::new(Vec::new()),
                            },
                            first.unwrap_or(start),
                        );
                    }
                    return Ok(None);
                }
                TokenKind::Keyword(keyword) => match keyword.as_slice() {
                    b"true" => Object::parsed(Value::Boolean(true), token.span),
                    b"false" => Object::parsed(Value::Boolean(false), token.span),
                    b"null" => Object::parsed(Value::Null, token.span),
                    b"BI" => {
                        return self
                            .inline_image(operands, first.unwrap_or(start), token.span, diagnostics)
                            .map(Some);
                    }
                    _ => {
                        return Ok(Some(Operation {
                            operator: keyword,
                            operands,
                            span: Span {
                                start: first.unwrap_or(start),
                                end: token.span.end,
                            },
                            inline_image: None,
                        }));
                    }
                },
                kind => self.operand(kind, token.span, 0, diagnostics)?,
            };
            first.get_or_insert(start);
            if operands.len() == MAX_OPERANDS {
                operands.remove(0);
                diagnostics.push(DiagnosticKind::OperandsDropped, start);
            }
            operands.push(object);
        }
    }

    /// The object a token begins: a scalar, or an array or dictionary read
    /// to its close.
    fn operand(
        &mut self,
        kind: TokenKind,
        span: Span,
        depth: usize,
        diagnostics: &mut Diagnostics,
    ) -> Result<Object, PdfError> {
        let value = match kind {
            TokenKind::Integer(integer) => Value::Integer(integer),
            TokenKind::Real(real) => Value::Real(real),
            TokenKind::String(string) => Value::String(string),
            TokenKind::Name(name) => Value::Name(name),
            TokenKind::ArrayOpen => {
                return self.array(span, depth, diagnostics);
            }
            TokenKind::DictOpen => {
                return self.dictionary(span, depth, diagnostics);
            }
            // A bracket or keyword where an operand should be, inside an
            // array or dictionary, is handled by the callers; here the
            // token is a close with nothing open.
            TokenKind::ArrayClose | TokenKind::DictClose => {
                diagnostics.push(DiagnosticKind::ContentByteSkipped, span.start);
                Value::Null
            }
            TokenKind::Keyword(_) | TokenKind::End => Value::Null,
        };
        Ok(Object::parsed(value, span))
    }

    fn array(
        &mut self,
        open: Span,
        depth: usize,
        diagnostics: &mut Diagnostics,
    ) -> Result<Object, PdfError> {
        let depth = depth + 1;
        if depth > MAX_NESTING_DEPTH {
            return Err(PdfError::LimitExceeded {
                offset: open.start,
                limit: Limit::NestingDepth,
            });
        }
        let mut items = Vec::new();
        loop {
            let token = self.lexer.next_token(diagnostics)?;
            match token.kind {
                TokenKind::ArrayClose => {
                    let span = Span {
                        start: open.start,
                        end: token.span.end,
                    };
                    return Ok(Object::parsed(Value::Array(items), span));
                }
                TokenKind::End => {
                    return Err(PdfError::UnexpectedEnd { offset: open.start });
                }
                TokenKind::Keyword(keyword) => match keyword.as_slice() {
                    b"true" => items.push(Object::parsed(Value::Boolean(true), token.span)),
                    b"false" => items.push(Object::parsed(Value::Boolean(false), token.span)),
                    b"null" => items.push(Object::parsed(Value::Null, token.span)),
                    // An operator inside an array: the array is unclosed.
                    // Give back what was read; the operator is read next.
                    _ => {
                        self.lexer.seek(token.span.start)?;
                        let span = Span {
                            start: open.start,
                            end: token.span.start,
                        };
                        diagnostics.push(DiagnosticKind::ContentByteSkipped, open.start);
                        return Ok(Object::parsed(Value::Array(items), span));
                    }
                },
                kind => items.push(self.operand(kind, token.span, depth, diagnostics)?),
            }
        }
    }

    fn dictionary(
        &mut self,
        open: Span,
        depth: usize,
        diagnostics: &mut Diagnostics,
    ) -> Result<Object, PdfError> {
        let depth = depth + 1;
        if depth > MAX_NESTING_DEPTH {
            return Err(PdfError::LimitExceeded {
                offset: open.start,
                limit: Limit::NestingDepth,
            });
        }
        let mut dictionary = Dictionary::new();
        loop {
            let key = self.lexer.next_token(diagnostics)?;
            match key.kind {
                TokenKind::DictClose => {
                    let span = Span {
                        start: open.start,
                        end: key.span.end,
                    };
                    return Ok(Object::parsed(Value::Dictionary(dictionary), span));
                }
                TokenKind::End => return Err(PdfError::UnexpectedEnd { offset: open.start }),
                TokenKind::Name(name) => {
                    let value = self.lexer.next_token(diagnostics)?;
                    let object = match value.kind {
                        TokenKind::DictClose => {
                            // A key with no value: the dictionary ends.
                            let span = Span {
                                start: open.start,
                                end: value.span.end,
                            };
                            diagnostics.push(DiagnosticKind::ContentByteSkipped, key.span.start);
                            return Ok(Object::parsed(Value::Dictionary(dictionary), span));
                        }
                        TokenKind::End => {
                            return Err(PdfError::UnexpectedEnd { offset: open.start })
                        }
                        TokenKind::Keyword(keyword) => match keyword.as_slice() {
                            b"true" => Object::parsed(Value::Boolean(true), value.span),
                            b"false" => Object::parsed(Value::Boolean(false), value.span),
                            _ => Object::parsed(Value::Null, value.span),
                        },
                        kind => self.operand(kind, value.span, depth, diagnostics)?,
                    };
                    dictionary.insert(name, object);
                }
                // A key that is not a name: skipped.
                _ => diagnostics.push(DiagnosticKind::ContentByteSkipped, key.span.start),
            }
        }
    }

    /// `BI`, the key-value pairs, `ID`, the data and `EI` (8.9.7).
    fn inline_image(
        &mut self,
        operands: Vec<Object>,
        start: u64,
        begin: Span,
        diagnostics: &mut Diagnostics,
    ) -> Result<Operation, PdfError> {
        let mut dictionary = Dictionary::new();
        let id = loop {
            let token = self.lexer.next_token(diagnostics)?;
            match token.kind {
                TokenKind::Keyword(ref keyword) if keyword == b"ID" => break token.span,
                TokenKind::Name(key) => {
                    let value = self.lexer.next_token(diagnostics)?;
                    let object = match value.kind {
                        TokenKind::Keyword(ref keyword) if keyword == b"ID" => {
                            // A key with no value before `ID`.
                            diagnostics.push(DiagnosticKind::InlineImageDamaged, begin.start);
                            return self.finish_damaged(operands, start, value.span);
                        }
                        TokenKind::Keyword(keyword) => match keyword.as_slice() {
                            b"true" => Object::parsed(Value::Boolean(true), value.span),
                            b"false" => Object::parsed(Value::Boolean(false), value.span),
                            _ => Object::parsed(Value::Null, value.span),
                        },
                        TokenKind::End => {
                            diagnostics.push(DiagnosticKind::InlineImageDamaged, begin.start);
                            return Ok(damaged(operands, start, token.span.end));
                        }
                        kind => self.operand(kind, value.span, 0, diagnostics)?,
                    };
                    dictionary.insert(expand_key(&key), expand_value(&key, object));
                }
                TokenKind::End => {
                    diagnostics.push(DiagnosticKind::InlineImageDamaged, begin.start);
                    return Ok(damaged(operands, start, token.span.end));
                }
                // Anything else before `ID` is not a pair.
                _ => diagnostics.push(DiagnosticKind::ContentByteSkipped, token.span.start),
            }
        };
        // One white-space byte follows `ID` (8.9.7); a carriage return
        // followed by a line feed counts as one.
        let size = self.source.len();
        let mut data_start = id.end;
        if data_start < size {
            let after = self.source.read(data_start, 1)?[0];
            if after == b'\r'
                && data_start + 1 < size
                && self.source.read(data_start + 1, 1)?[0] == b'\n'
            {
                data_start += 2;
            } else if is_whitespace(after) {
                data_start += 1;
            }
        }
        let (data_end, end) = match self.find_data_end(&dictionary, data_start)? {
            Some(found) => found,
            None => {
                diagnostics.push(DiagnosticKind::InlineImageDamaged, begin.start);
                self.lexer.seek(size)?;
                return Ok(damaged(operands, start, size));
            }
        };
        self.lexer.seek(end)?;
        Ok(Operation {
            operator: b"BI".to_vec(),
            operands,
            span: Span { start, end },
            inline_image: Some(InlineImage {
                dictionary,
                data: Span {
                    start: data_start,
                    end: data_end,
                },
            }),
        })
    }

    fn finish_damaged(
        &mut self,
        operands: Vec<Object>,
        start: u64,
        at: Span,
    ) -> Result<Operation, PdfError> {
        self.lexer.seek(at.end)?;
        Ok(damaged(operands, start, at.end))
    }

    /// Where the data of an inline image that starts at `data_start` ends,
    /// and where `EI` ends. `None` when there is no `EI`.
    fn find_data_end(
        &mut self,
        dictionary: &Dictionary,
        data_start: u64,
    ) -> Result<Option<(u64, u64)>, PdfError> {
        let size = self.source.len();
        // An unfiltered image has a length its dictionary fixes; a
        // `Length` entry (PDF 2.0) gives it for any.
        let exact = explicit_length(dictionary).or_else(|| unfiltered_length(dictionary));
        if let Some(len) = exact {
            if let Some(end) = data_start.checked_add(len).filter(|end| *end <= size) {
                if let Some(ei_end) = self.ei_after(end)? {
                    return Ok(Some((end, ei_end)));
                }
            }
        }
        // Otherwise `EI` set off by white space, and followed by what a
        // content stream holds, ends the data.
        let rest = usize::try_from(size - data_start).unwrap_or(usize::MAX);
        let bytes = self.source.read(data_start, rest)?;
        let mut index = 0;
        while index + 1 < bytes.len() {
            if bytes[index] == b'E'
                && bytes[index + 1] == b'I'
                && (index == 0 || is_whitespace(bytes[index - 1]))
                && ends_keyword(&bytes, index + 2)
                && follows_content(&bytes, index + 2)
            {
                // The white space before `EI` is not data (8.9.7 puts it
                // there to set the keyword off).
                let mut end = index;
                if end > 0 && is_whitespace(bytes[end - 1]) {
                    end -= 1;
                }
                let end = data_start + end as u64;
                let ei_end = data_start + index as u64 + 2;
                return Ok(Some((end, ei_end)));
            }
            index += 1;
        }
        Ok(None)
    }

    /// The end of an `EI` that follows `offset` after optional white
    /// space, when one does.
    fn ei_after(&self, offset: u64) -> Result<Option<u64>, PdfError> {
        let size = self.source.len();
        let rest = usize::try_from((size - offset).min(EI_LOOKAHEAD as u64 + 4)).unwrap_or(0);
        let bytes = self.source.read(offset, rest)?;
        let mut index = 0;
        while index < bytes.len() && is_whitespace(bytes[index]) {
            index += 1;
        }
        if bytes.get(index..index + 2) == Some(b"EI") && ends_keyword(&bytes, index + 2) {
            return Ok(Some(offset + index as u64 + 2));
        }
        Ok(None)
    }
}

fn damaged(operands: Vec<Object>, start: u64, end: u64) -> Operation {
    Operation {
        operator: b"BI".to_vec(),
        operands,
        span: Span { start, end },
        inline_image: None,
    }
}

/// Whether the byte at `index` (or the end) ends a keyword.
fn ends_keyword(bytes: &[u8], index: usize) -> bool {
    bytes
        .get(index)
        .is_none_or(|byte| is_whitespace(*byte) || is_delimiter(*byte))
}

/// Whether what follows an `EI` at `from` is content-stream text: no
/// control byte or byte past ASCII in the next few bytes.
fn follows_content(bytes: &[u8], from: usize) -> bool {
    let end = bytes.len().min(from + EI_LOOKAHEAD);
    bytes
        .get(from..end)
        .unwrap_or_default()
        .iter()
        .all(|byte| is_whitespace(*byte) || (0x20..0x7f).contains(byte))
}

fn number(object: &Object) -> Option<f64> {
    match object.value() {
        Value::Integer(integer) => Some(*integer as f64),
        Value::Real(real) => Some(real.get()),
        _ => None,
    }
}

/// `Length` of an inline image (ISO 32000-2, Table 92).
fn explicit_length(dictionary: &Dictionary) -> Option<u64> {
    match dictionary.get(b"Length")?.value() {
        Value::Integer(length) => u64::try_from(*length).ok(),
        _ => None,
    }
}

/// The data length of an unfiltered inline image whose colour space is
/// one of the device spaces or an Indexed space, or an image mask:
/// `ceil(width * components * bits / 8) * height` (8.9.3).
fn unfiltered_length(dictionary: &Dictionary) -> Option<u64> {
    if dictionary.get(b"Filter").is_some() {
        return None;
    }
    let width = u64::try_from(number(dictionary.get(b"Width")?)? as i64).ok()?;
    let height = u64::try_from(number(dictionary.get(b"Height")?)? as i64).ok()?;
    let is_mask = matches!(
        dictionary.get(b"ImageMask").map(Object::value),
        Some(Value::Boolean(true))
    );
    let (components, bits) = if is_mask {
        (1, 1)
    } else {
        let bits = u64::try_from(number(dictionary.get(b"BitsPerComponent")?)? as i64).ok()?;
        let components = match dictionary.get(b"ColorSpace")?.value() {
            Value::Name(name) => match name.as_bytes() {
                b"DeviceGray" => 1,
                b"DeviceRGB" => 3,
                b"DeviceCMYK" => 4,
                _ => return None,
            },
            Value::Array(items) => match items.first().map(Object::value) {
                Some(Value::Name(name)) if name.as_bytes() == b"Indexed" => 1,
                _ => return None,
            },
            _ => return None,
        };
        (components, bits)
    };
    let row_bits = width.checked_mul(components)?.checked_mul(bits)?;
    let row_bytes = row_bits.div_ceil(BITS_PER_BYTE);
    row_bytes.checked_mul(height)
}

/// The full key for an abbreviated one of Table 93; any other key as it is.
fn expand_key(key: &Name) -> Name {
    let full: &[u8] = match key.as_bytes() {
        b"BPC" => b"BitsPerComponent",
        b"CS" => b"ColorSpace",
        b"D" => b"Decode",
        b"DP" => b"DecodeParms",
        b"F" => b"Filter",
        b"H" => b"Height",
        b"IM" => b"ImageMask",
        b"I" => b"Interpolate",
        b"W" => b"Width",
        b"L" => b"Length",
        other => other,
    };
    Name::new(full)
}

/// The value with the abbreviations of Table 94 (colour spaces) and
/// Table 6 (filters) written out.
fn expand_value(key: &Name, value: Object) -> Object {
    let colour = matches!(key.as_bytes(), b"CS" | b"ColorSpace");
    let filter = matches!(key.as_bytes(), b"F" | b"Filter");
    if !colour && !filter {
        return value;
    }
    let expand = |item: Object| -> Object {
        let Value::Name(name) = item.value() else {
            return item;
        };
        let full: Option<&[u8]> = match (colour, name.as_bytes()) {
            (true, b"G") => Some(b"DeviceGray"),
            (true, b"RGB") => Some(b"DeviceRGB"),
            (true, b"CMYK") => Some(b"DeviceCMYK"),
            (true, b"I") => Some(b"Indexed"),
            (false, b"AHx") => Some(b"ASCIIHexDecode"),
            (false, b"A85") => Some(b"ASCII85Decode"),
            (false, b"LZW") => Some(b"LZWDecode"),
            (false, b"Fl") => Some(b"FlateDecode"),
            (false, b"RL") => Some(b"RunLengthDecode"),
            (false, b"CCF") => Some(b"CCITTFaxDecode"),
            (false, b"DCT") => Some(b"DCTDecode"),
            _ => None,
        };
        match full {
            Some(full) => Object::new(Value::Name(Name::new(full))),
            None => item,
        }
    };
    match value.into_value() {
        Value::Array(items) => {
            // A filter array expands every item; a colour space array only
            // its first (an Indexed base is a name too, expanded as well,
            // since Table 94 allows the abbreviations inside it).
            Object::new(Value::Array(items.into_iter().map(expand).collect()))
        }
        other => expand(Object::new(other)),
    }
}

/// An operand as a number, whatever its notation.
pub(crate) fn operand_number(object: &Object) -> Option<f64> {
    number(object)
}
