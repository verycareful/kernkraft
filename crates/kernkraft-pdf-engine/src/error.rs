// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Failures as an adapter's caller receives them.
//!
//! A failure is a `kind` and a `message`: `kind` is the name of the error
//! variant (`PageOutOfRange`, `WrongPassword`, `TextNotEditable`, ...), so the
//! application branches on it without parsing `message`. An error that only
//! forwards another crate's (`PdfError::Fs`, `PdfError::Font`) reports the
//! inner error's variant name with the outer error's message. The engine's
//! own failures are `BadRequest`, `UnknownHandle`, `SessionPoisoned`,
//! `StaleBlock`, `BufferTooSmall` and `RandomUnavailable`; an adapter adds
//! its own (`Panic` in `kernkraft-jni`).

use std::fmt::Debug;

use kernkraft_pdf::PdfError;
use serde::Serialize;

/// A failure as the application receives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WireError {
    pub kind: String,
    pub message: String,
}

impl WireError {
    pub fn new(kind: &str, message: impl Into<String>) -> Self {
        Self {
            kind: kind.to_owned(),
            message: message.into(),
        }
    }

    /// A request that is not what the operation takes, or names something
    /// that cannot exist.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new("BadRequest", message)
    }

    /// A block index read at an earlier revision than the document is at:
    /// the text it named may have moved, so the edit is refused.
    pub fn stale_block(read_at: u64, now: u64) -> Self {
        Self::new(
            "StaleBlock",
            format!(
                "the text block was read at revision {read_at} and the document is at revision {now}; read the page's text again"
            ),
        )
    }

    /// The buffer the application gave cannot hold the picture.
    pub fn buffer_too_small(needed: u64, given: u64) -> Self {
        Self::new(
            "BufferTooSmall",
            format!("the picture needs {needed} bytes and the buffer holds {given}"),
        )
    }

    pub fn unknown_handle(handle: i64) -> Self {
        Self::new(
            "UnknownHandle",
            format!("no open document has the handle {handle}"),
        )
    }

    /// A panic left the session's state in doubt, so it is refused rather
    /// than used; the application closes it and opens the file again.
    pub fn session_poisoned(handle: i64) -> Self {
        Self::new(
            "SessionPoisoned",
            format!("an earlier operation on the document with handle {handle} panicked"),
        )
    }
}

impl From<&PdfError> for WireError {
    fn from(error: &PdfError) -> Self {
        Self {
            kind: kind_of(error),
            message: error.to_string(),
        }
    }
}

impl From<&kernkraft_font::FontError> for WireError {
    fn from(error: &kernkraft_font::FontError) -> Self {
        Self {
            kind: variant_name(error),
            message: error.to_string(),
        }
    }
}

impl From<PdfError> for WireError {
    fn from(error: PdfError) -> Self {
        Self::from(&error)
    }
}

/// The variant name of `error`. The match has no catch-all arm, so a
/// variant added to `PdfError` stops this crate compiling until it is named.
pub fn kind_of(error: &PdfError) -> String {
    let own: &str = match error {
        PdfError::Fs(inner) => return variant_name(inner),
        PdfError::Font(inner) => return variant_name(inner),
        PdfError::Encrypted => "Encrypted",
        PdfError::NoPages => "NoPages",
        PdfError::PageOutOfRange { .. } => "PageOutOfRange",
        PdfError::DuplicatePage(..) => "DuplicatePage",
        PdfError::BadRange(..) => "BadRange",
        PdfError::Invalid(..) => "Invalid",
        PdfError::OutOfBounds { .. } => "OutOfBounds",
        PdfError::UnexpectedEnd { .. } => "UnexpectedEnd",
        PdfError::UnexpectedToken { .. } => "UnexpectedToken",
        PdfError::LimitExceeded { .. } => "LimitExceeded",
        PdfError::UnknownFilter { .. } => "UnknownFilter",
        PdfError::UnknownCMap { .. } => "UnknownCMap",
        PdfError::FilterParameters { .. } => "FilterParameters",
        PdfError::DecodedTooLarge { .. } => "DecodedTooLarge",
        PdfError::MissingHeader => "MissingHeader",
        PdfError::BadXref { .. } => "BadXref",
        PdfError::XrefMismatch { .. } => "XrefMismatch",
        PdfError::ObjectStream { .. } => "ObjectStream",
        PdfError::NoCatalog => "NoCatalog",
        PdfError::UnsupportedSecurityHandler { .. } => "UnsupportedSecurityHandler",
        PdfError::UnsupportedEncryption { .. } => "UnsupportedEncryption",
        PdfError::UnsupportedCryptFilter { .. } => "UnsupportedCryptFilter",
        PdfError::EncryptionDictionary { .. } => "EncryptionDictionary",
        PdfError::WrongPassword => "WrongPassword",
        PdfError::AttachmentsLocked => "AttachmentsLocked",
        PdfError::InvalidEntry { .. } => "InvalidEntry",
        PdfError::PageTree { .. } => "PageTree",
        PdfError::InvalidTree { .. } => "InvalidTree",
        PdfError::StructureTree { .. } => "StructureTree",
        PdfError::MissingObject { .. } => "MissingObject",
        PdfError::AnnotationNotOnPage { .. } => "AnnotationNotOnPage",
        PdfError::AnnotationNotEditable { .. } => "AnnotationNotEditable",
        PdfError::FontListRequired => "FontListRequired",
        PdfError::TextNotEditable { .. } => "TextNotEditable",
        PdfError::WrongKind { .. } => "WrongKind",
        PdfError::ObjectNumbersExhausted => "ObjectNumbersExhausted",
        PdfError::AttachmentDamaged { .. } => "AttachmentDamaged",
        PdfError::RevisionEncryption { .. } => "RevisionEncryption",
        PdfError::RebuiltNotEditable => "RebuiltNotEditable",
        PdfError::RevisionNotEditable => "RevisionNotEditable",
        PdfError::UnknownAppendPoint { .. } => "UnknownAppendPoint",
        PdfError::Function(..) => "Function",
        PdfError::ColourComponents { .. } => "ColourComponents",
        PdfError::NoGlyphForCharacter { .. } => "NoGlyphForCharacter",
        PdfError::Cancelled => "Cancelled",
    };
    own.to_owned()
}

/// The name of the variant a derived `Debug` prints: the identifier before
/// any field list.
pub fn variant_name(error: &dyn Debug) -> String {
    format!("{error:?}")
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect()
}
