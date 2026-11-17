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
    /// An encrypted file with no file identifier, which the key of
    /// security handler revisions 2 to 4 is computed from (7.6.3.3). An
    /// empty identifier is used.
    MissingFileId,
    /// A password that was accepted as its UTF-8 bytes and not in the form
    /// the security handler defines: PDFDocEncoding for revisions 2 to 4
    /// (7.6.3.3), the SASLprep form for revisions 5 and 6.
    PasswordEncoding,
    /// The `Perms` entry of an encryption dictionary of revision 5 or 6
    /// does not decrypt to the permissions and the `EncryptMetadata` the
    /// dictionary states. The stated values are used.
    PermissionsUnverified,
    /// AES-encrypted data that is not an initialization vector and whole
    /// blocks ending in padding (7.6.2). The whole blocks are decrypted
    /// and kept, with what stands where the padding should be.
    EncryptedDataDamaged,
    /// A stream of an encrypted file of revision 2 to 4 that names a
    /// crypt filter other than Identity in its `Filter`. 7.4.10 says such
    /// a stream is decrypted with the file key as it is; it is decrypted
    /// with the key of its object (Algorithm 1), as files are written.
    CryptFilterObjectKey,
    /// A page tree node whose `Type` is not `expected`: `Pages` for a node
    /// with `Kids`, `Page` for one without (7.7.3). It is read as what its
    /// `Kids` make it.
    PageTreeType { expected: Name },
    /// A `PageLabels` number tree with no range starting at page index 0,
    /// which 12.4.2 requires. The pages before its first range have no
    /// label.
    PageLabelsStartLate,
    /// An outline item reached a second time along the `First` and `Next`
    /// chains (12.3.3), so that the outline is no tree. The chain it was
    /// reached along ends there. The diagnostic's object is the item, or
    /// the outline dictionary, whose entry reached it.
    OutlineLoop,
    /// An entry of a document structure (an outline item, an embedded
    /// file's parameters) that is not of the type or within the values
    /// its table defines. The entry is left out of what is read; the
    /// module reading the structure says what that leaves.
    IgnoredEntry { key: Name },
    /// A file specification in the `EmbeddedFiles` name tree or a file
    /// attachment annotation that embeds no file (7.11.4). It is not
    /// listed as an attachment.
    AttachmentNotEmbedded,
    /// A named destination (12.3.2.3) that neither the catalog's `Dests`
    /// nor the `Dests` name tree defines, or a destination that is neither
    /// a name, a string nor an array. It leads nowhere.
    UnknownDestination,
    /// A destination that is not one of the forms of Table 151: a page,
    /// a view name and as many numbers as the view takes. It leads
    /// nowhere.
    InvalidDestination,
    /// An input of a merge was encrypted. The merged file is not: what an
    /// encryption dictionary says about who may do what is not carried
    /// over, and the application decides what to do about it.
    EncryptionNotCarried,
    /// An input of a merge had signatures. A signature covers the bytes
    /// of the file it was made in, so in the merged file it no longer
    /// verifies.
    SignaturesInvalidated,
    /// An input's `AcroForm` had an `XFA` entry (an XFA form, Annex H of
    /// ISO 32000-1:2008). The XML form is not merged; the fields of the
    /// `AcroForm` are.
    XfaNotCarried,
    /// An entry of an input's document catalog (or its trailer's `Info`)
    /// that a merge does not carry into the merged document.
    CatalogEntryNotCarried { key: Name },
    /// Two inputs of a merge give one name in a dictionary of default
    /// resources (`AcroForm` `DR`) to different objects. The first
    /// input's stays, so the second input's fields that use the name
    /// draw with the first's resource.
    ResourceNameKept { key: Name },
    /// Two inputs of a merge define one entry of the structure tree's
    /// `RoleMap`, `ClassMap` or `IDTree` differently. The first input's
    /// stays.
    StructureEntryKept { key: Name },
    /// A CMap entry that is not one: an operand of the wrong kind, a code
    /// of a length no CMap code has (1 to 4 bytes), a range whose ends
    /// differ in length or run backward, or a CID past 32 bits. The entry
    /// is skipped.
    CMapEntryInvalid,
    /// A CMap whose data ends inside a `begin`...`end` block. The entries
    /// read before the end are kept.
    CMapUnterminated,
    /// A CMap with two entries for one code. The first is kept.
    CMapRangesOverlap,
    /// A `ToUnicode` stream that cannot be read as a CMap: the font has no
    /// ToUnicode mapping.
    ToUnicodeUnreadable,
    /// An embedded font program that cannot be decoded or read, whose
    /// built-in encoding was therefore needed and is not known: the
    /// encoding is taken as the font would have it without a program.
    FontProgramUnreadable,
    /// An `Encoding` or `BaseEncoding` name that is none of the predefined
    /// encodings of Annex D: the implicit base encoding is used.
    EncodingUnknown { name: Name },
    /// An item of a `Differences` array that does not belong: a code
    /// outside 0 to 255, a name with no code before it, or an item of
    /// another kind. It is skipped.
    EncodingDifferenceInvalid,
    /// A simple font that is not one of the standard 14 and has no
    /// `Widths`. The descriptor's `MissingWidth` stands for every width.
    WidthsMissing,
    /// A `Widths` array with fewer items than `FirstChar` to `LastChar`
    /// has codes. The codes past its end have the missing width.
    WidthsShort,
    /// An item of `Widths`, `W` or `W2` that is not what the format needs.
    /// A `Widths` item has no width; the rest of a `W` or `W2` array from
    /// the item on is ignored.
    WidthInvalid,
    /// Two ranges of a `W` or `W2` array that cover a CID. The first
    /// gives its metrics.
    WidthsOverlap,
    /// A Type 0 font with more or fewer than one descendant font. The
    /// first is used.
    DescendantFontCount,
    /// A `CIDToGIDMap` that is not a stream: the identity map is used.
    CidToGidUnreadable,
    /// A byte in a content stream that starts no token (an unmatched `)`
    /// or `>`), skipped.
    ContentByteSkipped,
    /// More operands before one operator than the operand stack holds
    /// ([`MAX_OPERANDS`](crate::content::MAX_OPERANDS)): the earliest are
    /// dropped.
    OperandsDropped,
    /// An operator that is none of Table 51 (or one of the inline-image
    /// keywords outside an inline image). The operation is skipped.
    OperatorUnknown { operator: Name },
    /// An operator whose operands are too few or of the wrong kind
    /// (7.8.2). The operation is skipped.
    OperandsInvalid { operator: Name },
    /// An operator where the format does not allow it: a path segment
    /// with no current point, a text operator in the middle of a path,
    /// `BT` inside a text object. The operation is skipped unless its
    /// meaning is plain, and then it is done.
    OperatorMisplaced { operator: Name },
    /// A `Q` with no `q` before it. It is ignored.
    GraphicsStateUnderflow,
    /// A `q` with no `Q` before the end of its content stream. The state
    /// is restored as the stream ends.
    GraphicsStateUnclosed,
    /// A resource a content stream names that its resource dictionaries
    /// do not hold (`category` is `Font`, `XObject`, `ExtGState`,
    /// `ColorSpace`, `Pattern`, `Shading` or `Properties`). The
    /// operation is skipped.
    ResourceMissing { category: Name, name: Name },
    /// A form XObject that, directly or through others, paints itself.
    /// The inner painting is skipped.
    FormRecursion,
    /// An inline image whose data ends the stream before `EI`, or whose
    /// dictionary cannot be read. It is skipped.
    InlineImageDamaged,
    /// A `BMC` or `BDC` with no `EMC`, or an `EMC` with no `BMC` or
    /// `BDC`, in a content stream. The extra `EMC` is ignored and the
    /// unclosed sequences end with their stream.
    MarkedContentUnbalanced,
    /// A content stream that cannot be decoded. It is skipped.
    ContentStreamUnreadable,
    /// An optional-content group or membership dictionary that cannot be
    /// read. The content it governs is taken as visible.
    OptionalContentInvalid,
    /// A font in the `Font` resources that cannot be read as one. Text
    /// shown while it is the current font is skipped: no other font is
    /// put in its place.
    FontUnreadable { name: Name },
    /// A `Do` of an XObject whose `Subtype` is neither `Image` nor `Form`
    /// (a PostScript XObject, 8.8.2, or a damaged one). It is skipped.
    XObjectSubtypeUnsupported { subtype: Name },
    /// A glyph shown on a page that its font gives no Unicode text for.
    /// The text layer reads it as U+FFFD, the replacement character, so
    /// that the glyph keeps its place. Raised once for a page, at the
    /// first such glyph's operation.
    TextUnmapped,
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

    /// Takes back every diagnostic whose kind `keep` refuses.
    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&DiagnosticKind) -> bool) {
        self.list.retain(|diagnostic| keep(&diagnostic.kind));
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
