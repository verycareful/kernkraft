// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Loading and saving whole documents.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use kernkraft_fs::{write_bytes_atomic, FileStamp, FsError};
use lopdf::Document;

use crate::PdfError;

/// Reads and parses the PDF at `path`. See [`load_bytes`].
pub fn load(path: &Path) -> Result<Document, PdfError> {
    let bytes = std::fs::read(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            FsError::NotFound {
                path: path.to_path_buf(),
            }
        } else {
            FsError::Io {
                path: path.to_path_buf(),
                source: e,
            }
        }
    })?;
    load_bytes(&bytes)
}

/// Parses a PDF held in memory.
///
/// An encrypted file is [`PdfError::Encrypted`] even when the empty
/// password opens it, since saving it again would silently drop its
/// protection. A file with no pages is [`PdfError::NoPages`]. Anything the
/// parser rejects is [`PdfError::Parse`]; a panic inside the parser on a
/// malformed file is caught and reported the same way.
pub fn load_bytes(bytes: &[u8]) -> Result<Document, PdfError> {
    let parsed = catch_unwind(AssertUnwindSafe(|| Document::load_mem(bytes)))
        .map_err(|_| PdfError::Parse("the parser failed on malformed input".to_string()))?;
    let doc = parsed.map_err(|e| match e {
        lopdf::Error::Decryption(_)
        | lopdf::Error::InvalidPassword
        | lopdf::Error::UnsupportedSecurityHandler(_)
        | lopdf::Error::AlreadyEncrypted => PdfError::Encrypted,
        other => PdfError::Parse(other.to_string()),
    })?;
    if doc.is_encrypted() || doc.was_encrypted() {
        return Err(PdfError::Encrypted);
    }
    if page_count(&doc) == 0 {
        return Err(PdfError::NoPages);
    }
    Ok(doc)
}

/// Serialises `doc` to bytes.
pub fn to_bytes(doc: &mut Document) -> Result<Vec<u8>, PdfError> {
    let mut out = Vec::new();
    doc.save_to(&mut out)
        .map_err(|e| PdfError::Invalid(format!("cannot serialise the PDF: {e}")))?;
    Ok(out)
}

/// Serialises `doc` and writes it to `path` atomically. With
/// `expected = Some(stamp)` the save is refused if the file changed on disk
/// since `stamp` was taken. Returns the new stamp.
pub fn save(
    doc: &mut Document,
    path: &Path,
    expected: Option<&FileStamp>,
) -> Result<FileStamp, PdfError> {
    let bytes = to_bytes(doc)?;
    Ok(write_bytes_atomic(path, expected, &bytes)?)
}

/// Number of pages reachable through the page tree.
pub fn page_count(doc: &Document) -> u32 {
    u32::try_from(doc.page_iter().count()).unwrap_or(u32::MAX)
}
