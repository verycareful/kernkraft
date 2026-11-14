// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! What the reader accepted although the format forbids it or a portable
//! reader may not cope with it.
//!
//! Nothing is tolerated silently: each such decision is one [`Diagnostic`]
//! naming what was found, the object it was found in and the byte offset.
//! The portable limits are those of ISO 32000-1:2008, Annex C, Table C.1.

use crate::filter::FilterKind;
use crate::syntax::{Name, ObjectId};

/// A limit of Table C.1 that a conforming reader on a 32-bit machine should
/// accommodate. A value past one is read as it is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortableLimit {
    /// An integer outside the 32-bit signed range.
    Integer,
    /// A real of larger magnitude than a 32-bit float holds.
    Real,
    /// A name longer than 127 bytes.
    NameLength,
    /// An object number above 8,388,607.
    ObjectNumber,
}

/// What was found, and with it what the reader did.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum DiagnosticKind {
    /// A value past a portable limit, kept as written.
    BeyondPortableLimit(PortableLimit),
    /// An integer that does not fit 64 bits, read as a real.
    IntegerOverflow,
    /// A real with no finite 64-bit value, read as the largest finite one of
    /// its sign.
    RealOverflow,
    /// A number sign in a name that two hexadecimal digits do not follow,
    /// kept as itself.
    InvalidNameEscape,
    /// A zero byte written as `#00` in a name, kept.
    NulInName,
    /// A byte in a hexadecimal string that is neither a digit nor white
    /// space, skipped.
    InvalidHexDigit { byte: u8 },
    /// A key that a dictionary holds more than once. The last value is kept,
    /// in the place of the first.
    DuplicateKey { key: Name },
    /// A reference whose object number or generation is too large for any
    /// object to carry, read as null.
    ReferenceOutOfRange,
    /// An indirect object whose value neither `endobj` nor `stream` follows.
    /// The object ends after its value.
    MissingEndobj,
    /// A `stream` keyword followed by a lone carriage return or by no
    /// end-of-line. The data starts after the carriage return, or straight
    /// after the keyword.
    StreamKeywordEol,
    /// Encoded stream data that ends without its filter's end-of-data
    /// marker. Everything before the end is decoded and nothing is missing.
    MissingEndOfData { filter: FilterKind },
    /// Flate data whose Adler-32 checksum is absent or differs from the
    /// decoded bytes. Every block was decoded and all of it is kept.
    FlateChecksum,
    /// Encoded stream data that its filter cannot decode past some point.
    /// The bytes decoded before that point are kept, and the result is
    /// marked incomplete.
    FilterDataDamaged { filter: FilterKind },
    /// Bytes before the `%PDF-` header. Offsets in the file are read as
    /// written, counted from the first byte of the file.
    HeaderNotAtStart,
    /// No `%%EOF` after the `startxref` offset. The offset is used.
    MissingEofMarker,
    /// White space at the offset where the cross-reference or `startxref`
    /// places an object or a cross-reference section. What follows the
    /// white space is read.
    WhitespaceAtOffset,
    /// A cross-reference table entry that is not laid out as the 20 bytes
    /// of 7.5.4. Its three fields are read as tokens. Raised once for a
    /// section, at the first such entry.
    XrefEntryLayout,
    /// A cross-reference section with an entry for an object number at or
    /// past its trailer's `Size`, which Table 15 says to take as missing.
    /// The entry is kept: writers leave `Size` too low, the catalog's own
    /// number among those past it. Raised once for a section.
    EntryPastSize,
    /// A stream whose `Length` is absent, is not a whole number of bytes
    /// inside the file, or is not followed by `endstream`. The data is
    /// taken to end before the first `endstream` after its start, less
    /// one end-of-line.
    StreamLength,
    /// The cross-reference could not be read and was rebuilt from the
    /// objects found by reading the file through. The offset is where the
    /// cross-reference data was at fault.
    XrefRebuilt,
    /// An object stream that could not be read while the cross-reference
    /// was rebuilt. The objects in it are missing.
    ObjectStreamUnreadable { stream: u32 },
}

/// One tolerant decision.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub kind: DiagnosticKind,
    /// The indirect object being read, when there was one.
    pub object: Option<ObjectId>,
    /// The byte offset, in the source being read, of what was found.
    pub offset: u64,
}

/// The diagnostics of one read, in the order they were raised.
#[derive(Debug, Default)]
pub struct Diagnostics {
    list: Vec<Diagnostic>,
    object: Option<ObjectId>,
}

impl Diagnostics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a diagnostic against the object set by [`Self::set_object`].
    pub fn push(&mut self, kind: DiagnosticKind, offset: u64) {
        self.list.push(Diagnostic {
            kind,
            object: self.object,
            offset,
        });
    }

    /// Names the indirect object that diagnostics pushed from now on belong
    /// to, or `None` when no indirect object is being read.
    pub fn set_object(&mut self, object: Option<ObjectId>) {
        self.object = object;
    }

    pub fn as_slice(&self) -> &[Diagnostic] {
        &self.list
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn into_vec(self) -> Vec<Diagnostic> {
        self.list
    }

    /// How many diagnostics are held; with [`Self::truncate`], lets a reader
    /// take back what it raised while reading ahead.
    pub(crate) fn len(&self) -> usize {
        self.list.len()
    }

    pub(crate) fn truncate(&mut self, len: usize) {
        self.list.truncate(len);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    const OBJECT: ObjectId = ObjectId {
        number: 7,
        generation: 0,
    };

    #[test]
    fn push_keeps_order_and_offsets() {
        let mut diagnostics = Diagnostics::new();
        assert!(diagnostics.is_empty());
        diagnostics.push(DiagnosticKind::NulInName, 30);
        diagnostics.push(DiagnosticKind::MissingEndobj, 10);
        diagnostics.push(DiagnosticKind::InvalidHexDigit { byte: b'x' }, 20);
        let expected = [
            (DiagnosticKind::NulInName, 30),
            (DiagnosticKind::MissingEndobj, 10),
            (DiagnosticKind::InvalidHexDigit { byte: b'x' }, 20),
        ]
        .map(|(kind, offset)| Diagnostic {
            kind,
            object: None,
            offset,
        });
        assert_eq!(diagnostics.as_slice(), expected);
        assert_eq!(diagnostics.into_vec(), expected);
    }

    #[test]
    fn set_object_stamps_later_pushes_only() {
        let mut diagnostics = Diagnostics::new();
        diagnostics.push(DiagnosticKind::NulInName, 1);
        diagnostics.set_object(Some(OBJECT));
        diagnostics.push(DiagnosticKind::NulInName, 2);
        diagnostics.set_object(None);
        diagnostics.push(DiagnosticKind::NulInName, 3);
        let objects: Vec<Option<ObjectId>> =
            diagnostics.as_slice().iter().map(|d| d.object).collect();
        assert_eq!(objects, [None, Some(OBJECT), None]);
    }

    #[test]
    fn truncate_takes_back_the_latest() {
        let mut diagnostics = Diagnostics::new();
        diagnostics.push(DiagnosticKind::NulInName, 1);
        let mark = diagnostics.len();
        diagnostics.push(DiagnosticKind::MissingEndobj, 2);
        diagnostics.truncate(mark);
        assert_eq!(diagnostics.as_slice().len(), 1);
        assert_eq!(diagnostics.as_slice()[0].offset, 1);
    }
}
