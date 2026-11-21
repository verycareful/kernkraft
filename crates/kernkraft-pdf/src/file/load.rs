// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Reading one indirect object through the cross-reference: ISO
//! 32000-1:2008, 7.3.8.2 (stream extent, Table 5), 7.3.10 (indirect
//! objects), 7.5.4 and 7.5.8.3 (what an entry says of an object) and
//! 7.5.7 (compressed objects).

use std::borrow::Cow;

use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::file::object_stream::ObjectStream;
use crate::file::scan::find_forward;
use crate::file::xref::{lookup, Section, XrefEntry};
use crate::file::{is_damage, is_not_syntax, ReadLimits, StreamDecrypt};
use crate::filter::Pipeline;
use crate::syntax::lexer::unexpected;
use crate::syntax::{
    Dictionary, IndirectObject, Lexer, Object, ObjectId, Parser, Span, TokenKind, Value,
};
use crate::{PdfError, Source};

const LENGTH_KEY: &[u8] = b"Length";
const ENDSTREAM: &[u8] = b"endstream";
const ENDOBJ: &[u8] = b"endobj";

/// A stream object (7.3.8): its dictionary and where its data lies in the
/// file. The data is still encoded by the stream's filters, and encrypted
/// when the file is.
#[derive(Debug, Clone, PartialEq)]
pub struct Stream {
    pub dictionary: Dictionary,
    /// The bytes between `stream` and `endstream` that are the stream's
    /// data (Table 5, `Length`).
    pub data: Span,
}

/// What an indirect object holds.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    Object(Object),
    Stream(Stream),
}

/// Where an indirect object was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Location {
    /// In the file, at these bytes: from the object number to the end of
    /// `endobj`. The spans inside the object are offsets in the file.
    File(Span),
    /// In an object stream, at this index. The spans inside the object
    /// are offsets in the stream's decoded data.
    ObjectStream { stream: u32, index: u32 },
}

/// An indirect object as the file holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct FileObject {
    pub id: ObjectId,
    pub body: Body,
    pub location: Location,
}

/// Where the `Length` of a stream may be looked up when it is an indirect
/// reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LengthFrom {
    /// Nowhere: there is no cross-reference to find it with.
    Nowhere,
    /// Only an object that lies in the file itself. 7.5.7 forbids the
    /// length of an object stream to lie in an object stream.
    File,
    /// An object in the file or in an object stream.
    Anywhere,
}

/// Reads objects from a source through a list of cross-reference
/// sections, the newest first.
pub(crate) struct Reader<'a> {
    pub source: &'a dyn Source,
    pub sections: &'a [Section],
    pub limits: &'a ReadLimits,
    /// What decrypts an object stream before it is decoded, in an
    /// encrypted file whose key is known.
    pub decrypt: Option<&'a dyn StreamDecrypt>,
}

impl<'a> Reader<'a> {
    pub fn parser(&self) -> Parser<'a> {
        Parser::new(self.source).with_max_elements(self.limits.max_object_elements)
    }

    /// The object `id`, or `None` when the cross-reference has no such
    /// object in use: a reference to it is the null object (7.3.10).
    pub fn load(
        &self,
        id: ObjectId,
        diagnostics: &mut Diagnostics,
    ) -> Result<Option<FileObject>, PdfError> {
        match lookup(self.sections, id.number) {
            Some(XrefEntry::InUse { offset, generation }) if generation == id.generation => self
                .object_at(offset, id, LengthFrom::Anywhere, diagnostics)
                .map(Some),
            // 7.5.7: a compressed object has generation 0.
            Some(XrefEntry::Compressed { stream, index }) if id.generation == 0 => {
                let object = self.object_stream(stream, diagnostics)?.object(
                    index,
                    id.number,
                    self.limits.max_object_elements,
                    diagnostics,
                )?;
                Ok(Some(FileObject {
                    id,
                    body: Body::Object(object),
                    location: Location::ObjectStream { stream, index },
                }))
            }
            _ => Ok(None),
        }
    }

    /// The object stream `number`, decoded.
    pub fn object_stream(
        &self,
        number: u32,
        diagnostics: &mut Diagnostics,
    ) -> Result<ObjectStream, PdfError> {
        // 7.5.7: an object stream has generation 0 and is not itself a
        // compressed object.
        let Some(XrefEntry::InUse {
            offset,
            generation: 0,
        }) = lookup(self.sections, number)
        else {
            return Err(PdfError::ObjectStream { stream: number });
        };
        let id = ObjectId {
            number,
            generation: 0,
        };
        let object = self.object_at(offset, id, LengthFrom::File, diagnostics)?;
        self.decode_object_stream(&object, diagnostics)
    }

    /// Decodes `object` as an object stream.
    pub fn decode_object_stream(
        &self,
        object: &FileObject,
        diagnostics: &mut Diagnostics,
    ) -> Result<ObjectStream, PdfError> {
        let number = object.id.number;
        let Body::Stream(stream) = &object.body else {
            return Err(PdfError::ObjectStream { stream: number });
        };
        let stored = self.stream_data(stream)?;
        let encoded = match self.decrypt {
            Some(decrypt) => decrypt.decrypt_stream(
                object.id,
                &stream.dictionary,
                &stored,
                stream.data.start,
                diagnostics,
            )?,
            None => Cow::Borrowed(&*stored),
        };
        let pipeline = Pipeline::from_dictionary(&stream.dictionary)?;
        diagnostics.set_object(Some(object.id));
        let decoded = pipeline.decode(
            &encoded,
            self.limits.max_object_stream_bytes,
            stream.data.start,
            diagnostics,
        );
        diagnostics.set_object(None);
        let decoded = decoded?;
        if !decoded.complete {
            return Err(PdfError::ObjectStream { stream: number });
        }
        ObjectStream::parse(number, &stream.dictionary, decoded.data.into_owned())
    }

    /// The bytes of a stream's data as the file holds them.
    pub fn stream_data(&self, stream: &Stream) -> Result<Cow<'a, [u8]>, PdfError> {
        let Span { start, end } = stream.data;
        let len = end.saturating_sub(start);
        let len = usize::try_from(len).map_err(|_| PdfError::OutOfBounds {
            offset: start,
            len,
            size: self.source.len(),
        })?;
        self.source.read(start, len)
    }

    /// The indirect object at `offset`, which the cross-reference says is
    /// object `id`. Anything else there is [`PdfError::XrefMismatch`].
    pub fn object_at(
        &self,
        offset: u64,
        id: ObjectId,
        length_from: LengthFrom,
        diagnostics: &mut Diagnostics,
    ) -> Result<FileObject, PdfError> {
        let mismatch = || PdfError::XrefMismatch {
            number: id.number,
            generation: id.generation,
            offset,
        };
        let start = match self.header_at(offset) {
            Ok(Some((found, start))) if found == id => start,
            Ok(_) => return Err(mismatch()),
            Err(error) if is_not_syntax(&error) => return Err(mismatch()),
            Err(error) => return Err(error),
        };
        if start != offset {
            diagnostics.set_object(Some(id));
            diagnostics.push(DiagnosticKind::WhitespaceAtOffset, offset);
            diagnostics.set_object(None);
        }
        let mut parser = self.parser();
        parser.seek(start)?;
        let indirect = parser.parse_indirect_object(diagnostics)?;
        self.finish(indirect, length_from, diagnostics)
    }

    /// The identifier that the `obj` header at `offset` gives and where
    /// the header starts, or `None` when what is there is no such header.
    fn header_at(&self, offset: u64) -> Result<Option<(ObjectId, u64)>, PdfError> {
        let mut lexer = Lexer::new(self.source);
        lexer.seek(offset)?;
        // An object number is not judged as a plain integer.
        let mut unkept = Diagnostics::new();
        let number = lexer.next_token(&mut unkept)?;
        let generation = lexer.next_token(&mut unkept)?;
        let keyword = lexer.next_token(&mut unkept)?;
        let id = match (number.kind, generation.kind, keyword.kind) {
            (
                TokenKind::Integer(number),
                TokenKind::Integer(generation),
                TokenKind::Keyword(keyword),
            ) if keyword == b"obj" => u32::try_from(number)
                .ok()
                .zip(u16::try_from(generation).ok())
                .map(|(number, generation)| ObjectId { number, generation }),
            _ => None,
        };
        Ok(id.map(|id| (id, number.span.start)))
    }

    /// Completes an object the parser read: for a stream, finds where its
    /// data ends and reads on past `endstream` and `endobj`.
    pub fn finish(
        &self,
        indirect: IndirectObject,
        length_from: LengthFrom,
        diagnostics: &mut Diagnostics,
    ) -> Result<FileObject, PdfError> {
        let id = indirect.id;
        let Some(data_start) = indirect.stream_data_start else {
            return Ok(FileObject {
                id,
                body: Body::Object(indirect.object),
                location: Location::File(indirect.span),
            });
        };
        let object_start = indirect.span.start;
        let Value::Dictionary(dictionary) = indirect.object.into_value() else {
            // The parser reads `stream` only after a dictionary.
            return Err(unexpected(data_start, b"stream"));
        };
        let declared = self.declared_length(&dictionary, length_from, diagnostics)?;
        let declared_end = match declared.and_then(|length| data_start.checked_add(length)) {
            Some(end) if end <= self.source.len() => self
                .keyword_at(end, ENDSTREAM)?
                .map(|after_keyword| (end, after_keyword)),
            _ => None,
        };
        let (data_end, after_endstream) = match declared_end {
            Some(ends) => ends,
            None => {
                let ends = self.search_endstream(object_start, data_start)?;
                diagnostics.set_object(Some(id));
                diagnostics.push(DiagnosticKind::StreamLength, data_start);
                diagnostics.set_object(None);
                ends
            }
        };
        let object_end = match self.keyword_at(after_endstream, ENDOBJ)? {
            Some(end) => end,
            None => {
                diagnostics.set_object(Some(id));
                diagnostics.push(DiagnosticKind::MissingEndobj, after_endstream);
                diagnostics.set_object(None);
                after_endstream
            }
        };
        Ok(FileObject {
            id,
            body: Body::Stream(Stream {
                dictionary,
                data: Span {
                    start: data_start,
                    end: data_end,
                },
            }),
            location: Location::File(Span {
                start: object_start,
                end: object_end,
            }),
        })
    }

    /// The offset after `keyword`, when it is the next token at or after
    /// `offset`.
    fn keyword_at(&self, offset: u64, keyword: &[u8]) -> Result<Option<u64>, PdfError> {
        let mut lexer = Lexer::new(self.source);
        lexer.seek(offset)?;
        match lexer.next_token(&mut Diagnostics::new()) {
            Ok(token) => Ok(match token.kind {
                TokenKind::Keyword(found) if found == keyword => Some(token.span.end),
                _ => None,
            }),
            Err(error) if is_not_syntax(&error) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Where the data of a stream ends when its `Length` does not say: at
    /// the first `endstream` after its start, less one end-of-line
    /// (7.3.8.2: an end-of-line before `endstream` is not part of the
    /// data). Returns that offset and the offset after the keyword. A
    /// stream with no `endstream` is cut short by the end of the file.
    fn search_endstream(&self, object_start: u64, data_start: u64) -> Result<(u64, u64), PdfError> {
        let Some(keyword) = find_forward(self.source, ENDSTREAM, data_start)? else {
            return Err(PdfError::UnexpectedEnd {
                offset: object_start,
            });
        };
        let before = keyword.saturating_sub(2).max(data_start);
        let tail = self.source.read(before, (keyword - before) as usize)?;
        let end_of_line = match *tail {
            [.., b'\r', b'\n'] => 2,
            [.., b'\n' | b'\r'] => 1,
            _ => 0,
        };
        // `ENDSTREAM` is 9 bytes long.
        Ok((keyword - end_of_line, keyword + ENDSTREAM.len() as u64))
    }

    /// The value of `Length` (Table 5), or `None` when the dictionary has
    /// none that can be read as a count of bytes.
    fn declared_length(
        &self,
        dictionary: &Dictionary,
        length_from: LengthFrom,
        diagnostics: &mut Diagnostics,
    ) -> Result<Option<u64>, PdfError> {
        match dictionary.get(LENGTH_KEY).map(Object::value) {
            Some(Value::Integer(length)) => Ok(u64::try_from(*length).ok()),
            Some(Value::Reference(id)) => match self.length_object(*id, length_from, diagnostics) {
                Ok(length) => Ok(length),
                Err(error) if is_damage(&error) => Ok(None),
                Err(error) => Err(error),
            },
            _ => Ok(None),
        }
    }

    /// The integer that object `id` holds, for a `Length` that is an
    /// indirect reference.
    fn length_object(
        &self,
        id: ObjectId,
        length_from: LengthFrom,
        diagnostics: &mut Diagnostics,
    ) -> Result<Option<u64>, PdfError> {
        if length_from == LengthFrom::Nowhere {
            return Ok(None);
        }
        let object = match lookup(self.sections, id.number) {
            Some(XrefEntry::InUse { offset, generation }) if generation == id.generation => {
                let Some((found, start)) = self.header_at(offset)? else {
                    return Ok(None);
                };
                if found != id {
                    return Ok(None);
                }
                let mut parser = self.parser();
                parser.seek(start)?;
                let indirect = parser.parse_indirect_object(diagnostics)?;
                if indirect.stream_data_start.is_some() {
                    return Ok(None);
                }
                indirect.object
            }
            Some(XrefEntry::Compressed { stream, index })
                if id.generation == 0 && length_from == LengthFrom::Anywhere =>
            {
                self.object_stream(stream, diagnostics)?.object(
                    index,
                    id.number,
                    self.limits.max_object_elements,
                    diagnostics,
                )?
            }
            _ => return Ok(None),
        };
        Ok(match object.value() {
            Value::Integer(length) => u64::try_from(*length).ok(),
            _ => None,
        })
    }
}
