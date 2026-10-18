// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Loading and saving whole documents.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use kernkraft_fs::{write_bytes_atomic, FileStamp, FsError};
use lopdf::{Document, Object};

use crate::tree::{check_and_type_pages, page_ids};

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
/// parser rejects is [`PdfError::Parse`], and so is a malformed page tree
/// (cyclic, a node reached twice, nested too deeply); a panic inside the
/// parser on a malformed file is caught and reported the same way.
///
/// The document is normalised in two ways a save then keeps: a page-tree
/// node without `/Type` gets the one it must have, and the file's
/// cross-reference and object streams are dropped, since a save writes its
/// own.
pub fn load_bytes(bytes: &[u8]) -> Result<Document, PdfError> {
    let parsed = catch_unwind(AssertUnwindSafe(|| Document::load_mem(bytes)))
        .map_err(|_| PdfError::Parse("the parser failed on malformed input".to_string()))?;
    let mut doc = parsed.map_err(|e| match e {
        lopdf::Error::Decryption(_)
        | lopdf::Error::InvalidPassword
        | lopdf::Error::UnsupportedSecurityHandler(_)
        | lopdf::Error::AlreadyEncrypted => PdfError::Encrypted,
        other => PdfError::Parse(other.to_string()),
    })?;
    if doc.is_encrypted() || doc.was_encrypted() {
        return Err(PdfError::Encrypted);
    }
    drop_layout_streams(&mut doc);
    if check_and_type_pages(&mut doc)? == 0 {
        return Err(PdfError::NoPages);
    }
    Ok(doc)
}

/// Stream types that describe how the file was laid out rather than what
/// it holds: cross-reference streams and object streams (ISO 32000-1:2008,
/// 7.5.8 and 7.5.7). lopdf unpacks both on load but keeps them as objects;
/// every save writes fresh ones, so kept copies would be written again
/// under new numbers.
const LAYOUT_STREAM_TYPES: [&[u8]; 2] = [b"XRef", b"ObjStm"];

/// Removes the loaded layout streams and lowers `max_id` to the highest id
/// left, so a save numbers its own cross-reference stream as the file did.
fn drop_layout_streams(doc: &mut Document) {
    doc.objects.retain(|_, object| {
        object.as_stream().map_or(true, |s| {
            let kind = s.dict.get(b"Type").and_then(Object::as_name);
            !kind.is_ok_and(|k| LAYOUT_STREAM_TYPES.contains(&k))
        })
    });
    doc.max_id = doc.objects.keys().map(|&(id, _)| id).max().unwrap_or(0);
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

/// Number of pages in the page tree, counted as every operation counts
/// them: a kid without `/Kids` is a page whether or not it says
/// `/Type /Page`. A document from [`load`] or [`load_bytes`] always has a
/// well-formed tree; for one built by hand whose tree is malformed the
/// count is 0, and every operation on it reports [`PdfError::Parse`].
pub fn page_count(doc: &Document) -> u32 {
    page_ids(doc).map_or(0, |ids| u32::try_from(ids.len()).unwrap_or(u32::MAX))
}
