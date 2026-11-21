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
//!
//! An error that forwards a filesystem error also keeps what that error
//! knows beyond its message: the `path` it is about (an
//! `ExternalModification` names the file that changed under the session)
//! and, for the one variant a save can raise that carries a list, its
//! `details`.

use std::fmt::Debug;
use std::path::PathBuf;

use kernkraft_fs::FsError;
use kernkraft_pdf::PdfError;
use serde::Serialize;

/// A failure as the application receives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WireError {
    pub kind: String,
    pub message: String,
    /// The path the failure is about, when the variant names one.
    pub path: Option<PathBuf>,
    /// The structured parts of the variants that have any.
    pub details: Option<WireDetails>,
}

/// What a filesystem error carries beyond its path. JSON:
/// `{"k":"XattrsNotKept","names":[...]}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "k")]
pub enum WireDetails {
    /// [`FsError::XattrsNotKept`]: the attribute names a save could not
    /// carry over to the new file.
    XattrsNotKept { names: Vec<String> },
}

impl WireError {
    pub fn new(kind: &str, message: impl Into<String>) -> Self {
        Self {
            kind: kind.to_owned(),
            message: message.into(),
            path: None,
            details: None,
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
        let mut wire = Self::new(&kind_of(error), error.to_string());
        if let PdfError::Fs(inner) = error {
            wire.path = fs_path(inner);
            wire.details = fs_details(inner);
        }
        wire
    }
}

impl From<&kernkraft_font::FontError> for WireError {
    fn from(error: &kernkraft_font::FontError) -> Self {
        Self::new(&variant_name(error), error.to_string())
    }
}

/// The path `error` is about; none for a variant that is about no single
/// path (an undo that stopped partway, a name, a cancel). Same as the
/// application adapters report for the same variants. The match has no
/// catch-all arm, so a variant added to `FsError` stops this crate compiling
/// until it is placed.
fn fs_path(error: &FsError) -> Option<PathBuf> {
    match error {
        FsError::Io { path, .. }
        | FsError::ExternalModification { path }
        | FsError::Exists { path }
        | FsError::NotFound { path }
        | FsError::UnsafeCacheDir { path }
        | FsError::OwnershipNotKept { path }
        | FsError::XattrsNotKept { path, .. }
        | FsError::MountIdUnavailable { path }
        | FsError::MountPoint { path }
        | FsError::Changed { path, .. }
        | FsError::TrashUnavailable { path }
        | FsError::BadTrashInfo { path } => Some(path.clone()),
        FsError::IntoItself { src } => Some(src.clone()),
        FsError::SourceNotRemoved { left, .. } => Some(left.clone()),
        FsError::InvalidName { .. }
        | FsError::Cancelled
        | FsError::Irreversible { .. }
        | FsError::Partial { .. }
        | FsError::NothingToUndo
        | FsError::NothingToRedo => None,
    }
}

/// The details `error` carries beyond its path.
fn fs_details(error: &FsError) -> Option<WireDetails> {
    match error {
        FsError::XattrsNotKept { names, .. } => Some(WireDetails::XattrsNotKept {
            names: names.clone(),
        }),
        _ => None,
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
