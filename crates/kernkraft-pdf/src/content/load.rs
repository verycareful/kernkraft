// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Loading the bytes of a content stream: ISO 32000-1:2008, 7.8.2 (content
//! streams) and 7.7.3.3 (page objects, the `Contents` entry).
//!
//! The `Contents` of a page is one stream or an array of them. The
//! division between streams may fall only between tokens, and the streams
//! are read as one with white space between them, so they are joined here
//! by a line feed each; [`ContentStream::parts`] says where each began, for
//! an editor that must write a change back to the stream it came from.

use crate::content::device::StreamId;
use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::doc::{Content, Document};
use crate::error::PdfError;
use crate::syntax::{ObjectId, Value};

/// The most bytes a content stream decodes to when the caller does not say.
/// Large vector pages run to tens of megabytes decoded; this leaves room
/// above them while bounding what a small compressed stream can ask for.
pub const DEFAULT_MAX_CONTENT_BYTES: usize = 256 << 20;

/// The byte that joins the streams of a page's `Contents`.
const JOINER: u8 = b'\n';

/// One stream of a page's `Contents` and where it lies in
/// [`ContentStream::data`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentPart {
    pub object: ObjectId,
    /// The offset of the stream's first decoded byte in the joined data.
    pub start: usize,
    /// The stream's decoded length.
    pub len: usize,
}

/// A content stream, decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentStream {
    pub id: StreamId,
    /// The decoded bytes; for a page, its streams joined.
    pub data: Vec<u8>,
    /// The streams that make `data`: one for a stream of its own, one or
    /// more for a page. A stream that could not be decoded is not here.
    pub parts: Vec<ContentPart>,
}

impl ContentStream {
    /// The stream and the offset in it of the byte at `offset` of `data`,
    /// or `None` when it is a joining byte.
    pub fn locate(&self, offset: usize) -> Option<(ObjectId, usize)> {
        self.parts.iter().find_map(|part| {
            let end = part.start + part.len;
            (offset >= part.start && offset < end).then(|| (part.object, offset - part.start))
        })
    }
}

impl Document {
    /// The decoded bytes of the content stream `id`, at most `max_bytes` of
    /// each stream. The spans the interpreter reports are offsets into these
    /// bytes.
    ///
    /// A stream that cannot be decoded is skipped, with the diagnostic
    /// [`DiagnosticKind::ContentStreamUnreadable`] recorded on the document.
    /// One that decodes to more than `max_bytes` is
    /// [`PdfError::DecodedTooLarge`].
    pub fn content_stream(
        &self,
        id: StreamId,
        max_bytes: usize,
    ) -> Result<ContentStream, PdfError> {
        let mut diagnostics = Diagnostics::new();
        let result = self.load_content_stream(id, max_bytes, &mut diagnostics);
        self.record_diagnostics(diagnostics);
        result
    }

    pub(crate) fn load_content_stream(
        &self,
        id: StreamId,
        max_bytes: usize,
        diagnostics: &mut Diagnostics,
    ) -> Result<ContentStream, PdfError> {
        let streams = match id {
            StreamId::Object(object) => vec![object],
            StreamId::Page(page) => self.page_content_streams(page)?,
        };
        let mut data = Vec::new();
        let mut parts = Vec::new();
        for object in streams {
            let loaded = self.object(object)?;
            let Some(Content::Stream(stream)) = loaded.as_ref().map(|loaded| &loaded.content)
            else {
                diagnostics.set_object(Some(object));
                diagnostics.push(DiagnosticKind::ContentStreamUnreadable, 0);
                diagnostics.set_object(None);
                continue;
            };
            let decoded = match self.decode_stream(object, stream, max_bytes) {
                Ok(decoded) => decoded,
                // The caller set the cap, so a stream past it is its answer,
                // not damage to be skipped.
                Err(
                    error @ (PdfError::Fs(_)
                    | PdfError::Cancelled
                    | PdfError::DecodedTooLarge { .. }),
                ) => return Err(error),
                Err(_) => {
                    diagnostics.set_object(Some(object));
                    diagnostics.push(DiagnosticKind::ContentStreamUnreadable, 0);
                    diagnostics.set_object(None);
                    continue;
                }
            };
            if !parts.is_empty() {
                data.push(JOINER);
            }
            parts.push(ContentPart {
                object,
                start: data.len(),
                len: decoded.data.len(),
            });
            data.extend_from_slice(&decoded.data);
        }
        Ok(ContentStream { id, data, parts })
    }

    /// The streams of page `page`'s `Contents`, in order: none when the
    /// page has no contents.
    pub(crate) fn page_content_streams(&self, page: ObjectId) -> Result<Vec<ObjectId>, PdfError> {
        let loaded = self.required(page)?;
        let Some(dictionary) = loaded.dictionary() else {
            return Err(PdfError::PageTree { object: page });
        };
        let Some(entry) = dictionary.get(b"Contents") else {
            return Ok(Vec::new());
        };
        let invalid = || PdfError::InvalidEntry {
            object: Some(page),
            key: "Contents",
        };
        match entry.value() {
            Value::Reference(id) => {
                let target = self.required(*id)?;
                match &target.content {
                    Content::Stream(_) => Ok(vec![*id]),
                    Content::Object(object) => match object.value() {
                        Value::Array(items) => self.stream_references(items, invalid),
                        Value::Null => Ok(Vec::new()),
                        _ => Err(invalid()),
                    },
                }
            }
            Value::Array(items) => self.stream_references(items, invalid),
            Value::Null => Ok(Vec::new()),
            _ => Err(invalid()),
        }
    }

    fn stream_references(
        &self,
        items: &[crate::syntax::Object],
        invalid: impl Fn() -> PdfError,
    ) -> Result<Vec<ObjectId>, PdfError> {
        let mut streams = Vec::with_capacity(items.len());
        for item in items {
            match item.value() {
                Value::Reference(id) => streams.push(*id),
                Value::Null => {}
                _ => return Err(invalid()),
            }
        }
        Ok(streams)
    }
}
