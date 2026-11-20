// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

use std::fmt;

use crate::syntax::ObjectId;

use crate::colour::FunctionFault;
use crate::syntax::limits::{
    MAX_FILTERS, MAX_FORM_DEPTH, MAX_FUNCTION_DEPTH, MAX_GRAPHICS_STATE_DEPTH, MAX_NAME_BYTES,
    MAX_NESTING_DEPTH, MAX_STRING_BYTES, MAX_TOKEN_BYTES,
};

/// A hard limit of the reader and the writer. Input that passes one is
/// refused with [`PdfError::LimitExceeded`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    /// Arrays and dictionaries nested one inside another.
    NestingDepth,
    /// The bytes of one name.
    NameBytes,
    /// The bytes of one keyword or number.
    TokenBytes,
    /// The bytes of one string.
    StringBytes,
    /// The filters one stream chains.
    FilterCount,
    /// The `q` operators open at once in a content stream.
    GraphicsStateDepth,
    /// The form XObjects (and patterns and glyph procedures) open at once,
    /// one inside another.
    FormNesting,
    /// The functions open at once, one inside another.
    FunctionNesting,
    /// The colour spaces open at once, one inside another.
    ColourSpaceNesting,
    /// The inputs of a sampled function (7.10.2): each doubles the samples
    /// one evaluation reads.
    SampledInputs,
    /// The bytes of a sampled function's sample table, or of a PostScript
    /// calculator function's code.
    FunctionData,
    /// The operations one interpreter run executes, forms included. The
    /// caller sets `max` ([`InterpretOptions`](crate::content::InterpretOptions)).
    Operations { max: u64 },
    /// The array items and dictionary entries of one object, nested ones
    /// included. The caller sets `max`
    /// ([`ReadLimits`](crate::file::ReadLimits)).
    ObjectElements { max: u64 },
    /// The cross-reference entries of one file, over all its sections. The
    /// caller sets `max` ([`ReadLimits`](crate::file::ReadLimits)).
    XrefEntries { max: u64 },
    /// The glyphs one page shows, when its text is read. The caller sets
    /// `max` ([`TextOptions`](crate::TextOptions)).
    TextGlyphs { max: u64 },
    /// The vertices or patches of one mesh shading.
    MeshElements { max: u64 },
    /// The pixels of one render. The caller sets `max`
    /// ([`RenderOptions`](crate::render::RenderOptions)).
    RenderPixels { max: u64 },
    /// The pixels of one image, a mask or the picture itself. The caller
    /// sets `max` ([`ImageOptions`](crate::image::ImageOptions)).
    ImagePixels { max: u64 },
}

impl Limit {
    /// The largest value the limit allows.
    pub fn max(self) -> u64 {
        let max = match self {
            Limit::NestingDepth => MAX_NESTING_DEPTH,
            Limit::NameBytes => MAX_NAME_BYTES,
            Limit::TokenBytes => MAX_TOKEN_BYTES,
            Limit::StringBytes => MAX_STRING_BYTES,
            Limit::FilterCount => MAX_FILTERS,
            Limit::GraphicsStateDepth => MAX_GRAPHICS_STATE_DEPTH,
            Limit::FormNesting => MAX_FORM_DEPTH,
            Limit::FunctionNesting => MAX_FUNCTION_DEPTH,
            Limit::ColourSpaceNesting => crate::colour::MAX_COLOUR_SPACE_DEPTH,
            Limit::SampledInputs => crate::colour::MAX_SAMPLED_INPUTS,
            Limit::FunctionData => crate::colour::MAX_FUNCTION_BYTES,
            Limit::ObjectElements { max }
            | Limit::XrefEntries { max }
            | Limit::Operations { max }
            | Limit::TextGlyphs { max }
            | Limit::MeshElements { max }
            | Limit::RenderPixels { max }
            | Limit::ImagePixels { max } => return max,
        };
        // A usize is at most 64 bits wide on every supported target.
        max as u64
    }
}

impl fmt::Display for Limit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Limit::NestingDepth => "nesting depth",
            Limit::NameBytes => "name length",
            Limit::TokenBytes => "token length",
            Limit::StringBytes => "string length",
            Limit::FilterCount => "filter count",
            Limit::GraphicsStateDepth => "graphics state depth",
            Limit::FormNesting => "form nesting depth",
            Limit::FunctionNesting => "function nesting depth",
            Limit::ColourSpaceNesting => "colour space nesting depth",
            Limit::SampledInputs => "input count of a sampled function",
            Limit::FunctionData => "data size of a function",
            Limit::Operations { .. } => "operation count",
            Limit::ObjectElements { .. } => "element count of an object",
            Limit::XrefEntries { .. } => "cross-reference entry count",
            Limit::TextGlyphs { .. } => "glyph count of a page",
            Limit::MeshElements { .. } => "vertex or patch count of a mesh shading",
            Limit::RenderPixels { .. } => "pixel count of a render",
            Limit::ImagePixels { .. } => "pixel count of an image",
        })
    }
}

/// Every failure `kernkraft-pdf` reports.
#[derive(Debug, thiserror::Error)]
pub enum PdfError {
    #[error("the PDF is encrypted")]
    Encrypted,
    #[error("the PDF has no pages")]
    NoPages,
    #[error("page {page} is out of range 1-{count}")]
    PageOutOfRange { page: u32, count: u32 },
    #[error("page {0} is listed twice")]
    DuplicatePage(u32),
    #[error("bad page range {0:?}")]
    BadRange(String),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Fs(#[from] kernkraft_fs::FsError),
    /// A read that does not lie inside the source: its range starts or ends
    /// past the last byte, or its end does not fit in a `u64`.
    #[error("read of {len} bytes at offset {offset} is outside the file's {size} bytes")]
    OutOfBounds { offset: u64, len: u64, size: u64 },
    /// The bytes end inside a string, an array, a dictionary or an indirect
    /// object. `offset` is where the unfinished construct starts.
    #[error("the PDF ends inside a construct that starts at offset {offset}")]
    UnexpectedEnd { offset: u64 },
    /// A token that cannot stand where it was found. `found` holds the start
    /// of the token as text.
    #[error("unexpected {found} at offset {offset}")]
    UnexpectedToken { offset: u64, found: String },
    /// A construct that passes a hard limit. `offset` is where the construct
    /// starts in the source, or how many bytes had been written when the
    /// limit was passed while writing.
    #[error("{limit} at offset {offset} is above the limit of {}", .limit.max())]
    LimitExceeded { offset: u64, limit: Limit },
    /// A filter name that is not one this library decodes. `name` holds
    /// the start of the name as text.
    #[error("{name} is not a filter this library decodes")]
    UnknownFilter { name: String },
    /// The `Encoding` of a Type 0 font, or the `UseCMap` of a CMap, names
    /// a CMap that is not predefined (Table 118) and is no stream.
    #[error("{name} is not a predefined CMap")]
    UnknownCMap { name: String },
    /// A `Filter` or `DecodeParms` entry of the wrong shape, or a filter
    /// parameter outside the values its filter defines. `filter` is the
    /// filter's name, or `the stream` for the two entries themselves.
    #[error("the {key} of {filter} is not valid")]
    FilterParameters {
        filter: &'static str,
        key: &'static str,
    },
    /// Decoding a stream would give more bytes than the caller allowed.
    #[error("decoded data is larger than the {cap} bytes allowed")]
    DecodedTooLarge { cap: u64 },
    /// No `%PDF-` header with a version number in the first bytes of the
    /// file, where a reader looks for one.
    #[error("the file has no PDF header")]
    MissingHeader,
    /// Cross-reference data that cannot be read: `offset` is where the
    /// fault lies. Opening a file answers this by rebuilding the
    /// cross-reference, so it is returned only where that is not done.
    #[error("the cross-reference data at offset {offset} is not valid")]
    BadXref { offset: u64 },
    /// The cross-reference places an object at an offset where another
    /// object, or none, starts.
    #[error("object {number} {generation} is not at offset {offset}, where the cross-reference places it")]
    XrefMismatch {
        number: u32,
        generation: u16,
        offset: u64,
    },
    /// An object stream (7.5.7) whose dictionary or data cannot be read, or
    /// that does not hold the object the cross-reference places in it.
    #[error("object stream {stream} is not valid")]
    ObjectStream { stream: u32 },
    /// The cross-reference had to be rebuilt and nothing in the file names
    /// or is a document catalog.
    #[error("the PDF has no document catalog")]
    NoCatalog,
    /// The file is encrypted by a security handler other than the standard
    /// one (7.6.3), a public-key handler (7.6.4) among them. `name` holds
    /// the start of the handler's name as text.
    #[error("the PDF is encrypted by the security handler {name}, which is not supported")]
    UnsupportedSecurityHandler { name: String },
    /// The file is encrypted by the standard security handler with an
    /// algorithm (`V`) and revision (`R`) this library does not read
    /// together.
    #[error("the PDF is encrypted with algorithm {version} of revision {revision}, which is not supported")]
    UnsupportedEncryption { version: i64, revision: i64 },
    /// A crypt filter (7.6.5) that the encryption dictionary does not
    /// define, or whose method is not one the file's algorithm allows.
    /// `name` holds the start of the filter's name as text.
    #[error("the crypt filter {name} is not supported")]
    UnsupportedCryptFilter { name: String },
    /// An entry of the encryption dictionary (Tables 20 and 21), or the
    /// trailer's `Encrypt` itself, that is missing or is not what its
    /// table defines.
    #[error("the {key} of the encryption dictionary is not valid")]
    EncryptionDictionary { key: &'static str },
    /// The password is neither the user password nor the owner password
    /// of the file.
    #[error("the password does not open the PDF")]
    WrongPassword,
    /// Data of a file that encrypts only its embedded files, opened
    /// without a password, that cannot be read without one: an embedded
    /// file, or anything else not left plain. The application asks for the
    /// password and calls
    /// [`Document::unlock_attachments`](crate::doc::Document::unlock_attachments).
    #[error("the attachments of the PDF need its password")]
    AttachmentsLocked,
    /// An entry of a document structure (the catalog, a page, a tree
    /// node, the trailer) that is missing where the format requires it,
    /// or is not of the type its table defines. `object` is the indirect
    /// object holding the entry, when there is one.
    #[error("the {key} entry of {} is not valid", describe(.object))]
    InvalidEntry {
        object: Option<ObjectId>,
        key: &'static str,
    },
    /// A page tree (7.7.3) in which `object` is reached twice, so that the
    /// tree is not a tree, or is a node that is not a dictionary.
    #[error("the page tree is not valid at object {} {}", .object.number, .object.generation)]
    PageTree { object: ObjectId },
    /// A name tree or number tree (7.9.6, 7.9.7) in which a node is
    /// reached twice, or that holds a node or an entry of the wrong type.
    #[error("the name or number tree is not valid at {}", describe(.object))]
    InvalidTree { object: Option<ObjectId> },
    /// An object that the document's cross-reference or edits do not have,
    /// where one is required.
    #[error("the structure tree is not valid at object {} {}", .object.number, .object.generation)]
    StructureTree { object: ObjectId },
    #[error("object {} {} is missing", .object.number, .object.generation)]
    MissingObject { object: ObjectId },
    /// An annotation asked for by a page that does not list it in its
    /// `Annots` (12.5.2, Table 30).
    #[error("object {} {} is not an annotation of page {page}", .object.number, .object.generation)]
    AnnotationNotOnPage { object: ObjectId, page: u32 },
    /// A change to an annotation whose kind the library does not write
    /// or take apart: its appearance cannot be made again, or it belongs
    /// to a form field.
    #[error("a {subtype} annotation cannot be changed this way")]
    AnnotationNotEditable { subtype: String },
    /// A stream whose data was asked for as an object, or the other way
    /// round.
    #[error("object {} {} is not of the kind expected", .object.number, .object.generation)]
    WrongKind { object: ObjectId },
    /// An operation ran out of object numbers: the next would pass the
    /// largest a cross-reference can hold.
    #[error("no object number is left")]
    ObjectNumbersExhausted,
    /// An attachment whose data could not all be decoded, or does not
    /// agree with the size or checksum its parameters state (7.11.4,
    /// Table 46), asked to be saved to a file.
    #[error("the attachment in stream {} {} is damaged", .stream.number, .stream.generation)]
    AttachmentDamaged { stream: ObjectId },
    /// An earlier revision whose trailer names other encryption than the
    /// file's newest trailer, which the key the file was opened with does
    /// not read.
    #[error("revision {revision} is encrypted otherwise than the file")]
    RevisionEncryption { revision: usize },
    /// An edit session over a file whose cross-reference was rebuilt: the
    /// bytes the rebuilt reading took for objects are not what an
    /// incremental update can be appended to, so such a file is saved only
    /// as a compacted copy.
    #[error("the file's cross-reference was rebuilt, so it can be saved only as a compacted copy")]
    RebuiltNotEditable,
    /// An edit session over an earlier revision of a file, which an
    /// incremental update would append to the end of the whole file, past
    /// the later revisions.
    #[error("an earlier revision of a file cannot be edited in place")]
    RevisionNotEditable,
    /// Bytes appended since a length that no save of the session ended at.
    #[error("no save of the session left the file {length} bytes long")]
    UnknownAppendPoint { length: u64 },
    /// A function (7.10) that could not be evaluated for the input given,
    /// or that failed while it ran.
    #[error("a function cannot be evaluated: {0}")]
    Function(FunctionFault),
    /// A colour given in a colour space that has a different number of
    /// components.
    #[error("a colour of {found} components was given to a colour space of {expected}")]
    ColourComponents { expected: usize, found: usize },
    /// The operation was cancelled through its progress handle.
    #[error("the operation was cancelled")]
    Cancelled,
}

fn describe(object: &Option<ObjectId>) -> String {
    match object {
        Some(id) => format!("object {} {}", id.number, id.generation),
        None => "a direct object".to_string(),
    }
}
