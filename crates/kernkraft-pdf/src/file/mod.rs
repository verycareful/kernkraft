// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The structure of a PDF file: ISO 32000-1:2008, 7.5.
//!
//! [`PdfFile::open`] reads the header (7.5.2) and the cross-reference
//! sections with their trailers (7.5.4, 7.5.5, 7.5.8), newest first, each
//! incremental update adding one (7.5.6). [`PdfFile::load`] then reads
//! one indirect object where the cross-reference places it: in the file,
//! or in an object stream (7.5.7). Nothing else of the body is read, so
//! opening a file costs its cross-reference and no more.
//!
//! A file whose cross-reference cannot be read is opened by reading it
//! through for its objects ([`PdfFile::rebuild`]); the
//! [`XrefRebuilt`](crate::DiagnosticKind::XrefRebuilt) diagnostic and
//! [`PdfFile::xref_rebuilt`] say so.
//!
//! Objects are returned as the file holds them. Stream data is not
//! decoded and nothing is decrypted.

mod chain;
mod header;
mod load;
mod object_stream;
mod repair;
mod scan;
mod xref;

use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;

pub use chain::TRAILER_SEARCH_BYTES;
pub use header::{Version, HEADER_SEARCH_BYTES};
pub use load::{Body, FileObject, Location, Stream};
pub use object_stream::ObjectStream;
pub use xref::{Section, SectionForm, XrefEntry};

use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::syntax::{Dictionary, ObjectId, Value};
use crate::{Limit, PdfError, Source};
use load::Reader;

const ROOT_KEY: &[u8] = b"Root";

/// The limits a caller sets on what reading one file may cost in memory.
/// The format sets none of them, and what is reasonable depends on the
/// device, so there is no default. Input that passes one is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadLimits {
    /// The most array items and dictionary entries one object may hold,
    /// nested ones included. Passing it is
    /// [`Limit::ObjectElements`](crate::Limit::ObjectElements).
    pub max_object_elements: usize,
    /// The most cross-reference entries the file may have, over all its
    /// sections. Passing it is
    /// [`Limit::XrefEntries`](crate::Limit::XrefEntries).
    pub max_xref_entries: usize,
    /// The most bytes the decoded data of one object stream may have.
    /// Passing it is [`PdfError::DecodedTooLarge`].
    pub max_object_stream_bytes: usize,
}

/// An open PDF file: its header and its cross-reference. Immutable, and
/// shared between threads freely.
pub struct PdfFile {
    source: Arc<dyn Source>,
    limits: ReadLimits,
    version: Version,
    header_offset: u64,
    /// The cross-reference sections, the newest first. Never empty.
    sections: Vec<Section>,
}

impl fmt::Debug for PdfFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PdfFile")
            .field("source_len", &self.source.len())
            .field("limits", &self.limits)
            .field("version", &self.version)
            .field("header_offset", &self.header_offset)
            .field("sections", &self.sections)
            .finish()
    }
}

impl PdfFile {
    /// Reads the header and the cross-reference of `source`.
    ///
    /// When the cross-reference cannot be read, or its newest trailer
    /// names no `Root`, the file is opened as [`Self::rebuild`] opens it.
    /// A file with no header is [`PdfError::MissingHeader`].
    pub fn open(
        source: Arc<dyn Source>,
        limits: ReadLimits,
        diagnostics: &mut Diagnostics,
    ) -> Result<Self, PdfError> {
        let header = header::read_header(&*source, diagnostics)?;
        let mark = diagnostics.len();
        let fault = match chain::read_chain(&*source, &limits, diagnostics) {
            Ok(sections) => match sections.first() {
                Some(newest) if names_root(newest.trailer()) => {
                    return Ok(Self {
                        source,
                        limits,
                        version: header.version,
                        header_offset: header.offset,
                        sections,
                    });
                }
                newest => newest.map_or(header.offset, Section::offset),
            },
            Err(error) if is_not_syntax(&error) => fault_offset(&error).unwrap_or(header.offset),
            Err(error) => return Err(error),
        };
        // What was said of a cross-reference that is not used is not kept.
        diagnostics.truncate(mark);
        Self::rebuilt(source, limits, header, fault, diagnostics)
    }

    /// Opens `source` without its cross-reference: the file is read
    /// through, every indirect object found is taken to be where it is,
    /// and of two with one object number the later in the file counts.
    /// The trailer is put together from the trailers and cross-reference
    /// streams found, or names the catalog found.
    ///
    /// For a file that [`Self::open`] opened but whose cross-reference
    /// turned out wrong ([`PdfError::XrefMismatch`]). A file in which no
    /// catalog is found is [`PdfError::NoCatalog`].
    pub fn rebuild(
        source: Arc<dyn Source>,
        limits: ReadLimits,
        diagnostics: &mut Diagnostics,
    ) -> Result<Self, PdfError> {
        let header = header::read_header(&*source, diagnostics)?;
        Self::rebuilt(source, limits, header, header.offset, diagnostics)
    }

    fn rebuilt(
        source: Arc<dyn Source>,
        limits: ReadLimits,
        header: header::Header,
        fault: u64,
        diagnostics: &mut Diagnostics,
    ) -> Result<Self, PdfError> {
        diagnostics.push(DiagnosticKind::XrefRebuilt, fault);
        let section = repair::rebuild(&*source, header.offset, &limits, diagnostics)?;
        Ok(Self {
            source,
            limits,
            version: header.version,
            header_offset: header.offset,
            sections: vec![section],
        })
    }

    /// The source the file is read from.
    pub fn source(&self) -> &Arc<dyn Source> {
        &self.source
    }

    pub fn limits(&self) -> ReadLimits {
        self.limits
    }

    /// The version the header names (7.5.2).
    pub fn version(&self) -> Version {
        self.version
    }

    /// Where the header starts: 0 in a file written as 7.5.2 says.
    pub fn header_offset(&self) -> u64 {
        self.header_offset
    }

    /// The cross-reference sections, the newest first: the last
    /// incremental update, back to the original section of the file. A
    /// file whose cross-reference was rebuilt has one.
    pub fn sections(&self) -> &[Section] {
        &self.sections
    }

    /// Whether the cross-reference was rebuilt by reading the file
    /// through.
    pub fn xref_rebuilt(&self) -> bool {
        self.sections
            .first()
            .is_some_and(|section| section.form() == SectionForm::Rebuilt)
    }

    /// The trailer dictionary of the newest section.
    pub fn trailer(&self) -> &Dictionary {
        // `sections` is never empty; an empty dictionary stands in so
        // that this cannot fail.
        static EMPTY: std::sync::OnceLock<Dictionary> = std::sync::OnceLock::new();
        match self.sections.first() {
            Some(section) => section.trailer(),
            None => EMPTY.get_or_init(Dictionary::new),
        }
    }

    /// What the cross-reference says of object `number`: the entry of the
    /// newest section that has one (7.5.6). `None` for a number no
    /// section has.
    pub fn entry(&self, number: u32) -> Option<XrefEntry> {
        self.entry_as_of(0, number)
    }

    /// As [`Self::entry`], for the file as it was when section `section`
    /// of [`Self::sections`] was its newest.
    pub fn entry_as_of(&self, section: usize, number: u32) -> Option<XrefEntry> {
        xref::lookup(self.sections.get(section..)?, number)
    }

    /// Reads object `id`. `None` when the file has no such object in use:
    /// a reference to it is the null object (7.3.10).
    ///
    /// An object that is not where the cross-reference places it is
    /// [`PdfError::XrefMismatch`]; [`Self::rebuild`] opens such a file.
    pub fn load(
        &self,
        id: ObjectId,
        diagnostics: &mut Diagnostics,
    ) -> Result<Option<FileObject>, PdfError> {
        self.load_as_of(0, id, diagnostics)
    }

    /// As [`Self::load`], for the file as it was when section `section`
    /// of [`Self::sections`] was its newest. A section the file does not
    /// have holds no object.
    pub fn load_as_of(
        &self,
        section: usize,
        id: ObjectId,
        diagnostics: &mut Diagnostics,
    ) -> Result<Option<FileObject>, PdfError> {
        match self.reader(section) {
            Some(reader) => reader.load(id, diagnostics),
            None => Ok(None),
        }
    }

    /// Reads and decodes object stream `number` (7.5.7), for reading
    /// several of its objects with [`ObjectStream::object`] at the cost
    /// of one decoding.
    pub fn object_stream(
        &self,
        number: u32,
        diagnostics: &mut Diagnostics,
    ) -> Result<ObjectStream, PdfError> {
        match self.reader(0) {
            Some(reader) => reader.object_stream(number, diagnostics),
            None => Err(PdfError::ObjectStream { stream: number }),
        }
    }

    /// The data of `stream` as the file holds it: encoded by the stream's
    /// filters, and encrypted when the file is. All of it is read into
    /// memory when the source is a file; a caller that wants a part reads
    /// [`Stream::data`]'s range from [`Self::source`].
    pub fn stream_data(&self, stream: &Stream) -> Result<Cow<'_, [u8]>, PdfError> {
        Reader {
            source: &*self.source,
            sections: &self.sections,
            limits: &self.limits,
        }
        .stream_data(stream)
    }

    fn reader(&self, section: usize) -> Option<Reader<'_>> {
        Some(Reader {
            source: &*self.source,
            sections: self.sections.get(section..).filter(|s| !s.is_empty())?,
            limits: &self.limits,
        })
    }
}

/// Whether a trailer names the catalog as Table 15 requires: `Root` is an
/// indirect reference.
fn names_root(trailer: &Dictionary) -> bool {
    matches!(
        trailer.get(ROOT_KEY).map(|object| object.value()),
        Some(Value::Reference(_))
    )
}

/// Whether `error` says that bytes of the file are not what the format
/// has there, as opposed to a failed read or a limit passed.
pub(crate) fn is_damage(error: &PdfError) -> bool {
    matches!(
        error,
        PdfError::BadXref { .. }
            | PdfError::UnexpectedEnd { .. }
            | PdfError::UnexpectedToken { .. }
            | PdfError::OutOfBounds { .. }
            | PdfError::XrefMismatch { .. }
            | PdfError::ObjectStream { .. }
            | PdfError::UnknownFilter { .. }
            | PdfError::FilterParameters { .. }
    )
}

/// As [`is_damage`], and also true of bytes that pass one of the syntax
/// layer's own limits: where a cross-reference or an object was expected
/// and the bytes are not one, they can be anything.
pub(crate) fn is_not_syntax(error: &PdfError) -> bool {
    match error {
        PdfError::LimitExceeded { limit, .. } => !matches!(
            limit,
            Limit::ObjectElements { .. } | Limit::XrefEntries { .. }
        ),
        other => is_damage(other),
    }
}

/// Where in the file `error` places its fault, when it places one.
fn fault_offset(error: &PdfError) -> Option<u64> {
    match error {
        PdfError::BadXref { offset }
        | PdfError::UnexpectedEnd { offset }
        | PdfError::UnexpectedToken { offset, .. }
        | PdfError::OutOfBounds { offset, .. }
        | PdfError::LimitExceeded { offset, .. }
        | PdfError::XrefMismatch { offset, .. } => Some(*offset),
        _ => None,
    }
}
