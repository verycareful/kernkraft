// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The full writer: a document written as a new file with one
//! cross-reference section, ISO 32000-1:2008, 7.5 (file structure).
//!
//! What is written is what the trailer reaches (7.5.5): the catalog and
//! everything it refers to, the document information, the encryption
//! dictionary. Objects keep their numbers and generations, so nothing that
//! refers to them changes; a number nothing reaches becomes a free entry
//! (7.5.4). An object read from the file without anything tolerated is
//! written as its original bytes; an edited object, and one whose reading
//! tolerated something, is written in the canonical form of
//! [`write_object`](crate::syntax::write_object).
//!
//! The cross-reference takes the form the file had: a file with object
//! streams or a cross-reference stream is written with a cross-reference
//! stream (7.5.8), the objects that were compressed and the new ones that
//! can be put in object streams (7.5.7); any other file is written with a
//! table (7.5.4). An encrypted file stays encrypted with the same key and
//! the same first file identifier (7.6, 14.4).
//!
//! The same document and options always give the same bytes: the second
//! file identifier and the initialization vectors of AES come from the
//! caller's seed.

mod objects;
mod renumber;
mod xref;

use std::path::Path;

use kernkraft_fs::{write_bytes_atomic, Expect, FileStamp, Progress};

use crate::crypt::{IvSource, SEED_BYTES};
use crate::doc::Document;
use crate::file::{SectionForm, XrefEntry};
use crate::syntax::{Dictionary, Name, Object, ObjectId, PdfString, Value};
use crate::PdfError;
pub(crate) use objects::reachable;
use objects::{Placement, Writer};

const ID_KEY: &[u8] = b"ID";
const ENCRYPT_KEY: &[u8] = b"Encrypt";
const SIZE_KEY: &[u8] = b"Size";

/// Trailer entries that describe one cross-reference section and are
/// written anew (Tables 15 and 17): `Size`, `Prev`, `XRefStm`, and the
/// entries of a cross-reference stream's own dictionary.
const SECTION_KEYS: [&[u8]; 13] = [
    b"Size",
    b"Prev",
    b"XRefStm",
    b"Type",
    b"W",
    b"Index",
    b"Length",
    b"Filter",
    b"DecodeParms",
    b"F",
    b"FFilter",
    b"FDecodeParms",
    b"DL",
];

/// The bytes of a file identifier this writer makes (14.4 asks for 16, the
/// size of an MD5 digest).
const ID_BYTES: usize = 16;
/// What the second identifier is derived from with the seed, so that it is
/// never the same bytes as an initialization vector.
const ID_LABEL: &[u8] = b"kernkraft-pdf file identifier";

/// What a write takes from the caller, so that it gives the same bytes
/// every time it is given the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteOptions {
    /// Fresh random bytes for each write, from which the file identifier
    /// and the initialization vectors of AES are derived. Writing two
    /// files with one seed gives them the same identifier.
    pub seed: [u8; SEED_BYTES],
}

/// The bytes of `document` written as a new file. `progress` counts
/// objects; a cancelled write is [`PdfError::Cancelled`].
pub fn compact(
    document: &Document,
    options: &WriteOptions,
    progress: &dyn Progress,
) -> Result<Vec<u8>, PdfError> {
    let (bytes, numbers) = write_whole(document, options, progress)?;
    let highest = numbers.last().map_or(0, |id| id.number);
    if !renumber::is_sparse(highest, numbers.len(), bytes.len()) {
        return Ok(bytes);
    }
    let dense = renumber::dense(document, &numbers)?;
    Ok(write_whole(&dense, options, progress)?.0)
}

/// The bytes of `document` with its objects under their own numbers, and
/// the objects written, in order of number.
fn write_whole(
    document: &Document,
    options: &WriteOptions,
    progress: &dyn Progress,
) -> Result<(Vec<u8>, Vec<ObjectId>), PdfError> {
    let trailer = document.trailer();
    let numbers = reachable(document, trailer, progress)?;
    let streams = uses_object_streams(document);
    let mut ivs = IvSource::new(options.seed);
    let mut writer = Writer::new(document, &mut ivs)?;
    // The writer's own objects (object streams, the cross-reference
    // stream) follow the highest number it writes.
    writer.next_number = numbers
        .last()
        .map_or(1, |id| id.number.saturating_add(1))
        .max(1);
    writer.header();
    let total = numbers.len() as u64;
    let mut placed = Vec::with_capacity(numbers.len());
    for (done, id) in numbers.iter().enumerate() {
        if progress.is_cancelled() {
            return Err(PdfError::Cancelled);
        }
        progress.report(done as u64, total, "objects");
        let object = document.required(*id)?;
        placed.push((object.id, writer.place(&object, streams)?));
    }
    let mut entries: Vec<(u32, XrefEntry)> = Vec::with_capacity(placed.len());
    let mut compressed = Vec::new();
    for (id, placement) in placed {
        match placement {
            Placement::File { offset } => entries.push((
                id.number,
                XrefEntry::InUse {
                    offset,
                    generation: id.generation,
                },
            )),
            Placement::Compressible(bytes) => compressed.push((id.number, bytes)),
        }
    }
    entries.extend(writer.object_streams(compressed)?);
    let trailer = written_trailer(document, trailer, options);
    let bytes = if streams {
        xref::stream(writer, entries, trailer)?
    } else {
        xref::table(writer, entries, trailer)
    };
    progress.report(total, total, "objects");
    Ok((bytes, numbers))
}

/// Writes `document` as [`compact`] does to `path`, atomically as far as
/// `expect` allows. Returns the new stamp.
pub fn save_compact(
    document: &Document,
    path: &Path,
    expect: Expect<'_>,
    options: &WriteOptions,
    progress: &dyn Progress,
) -> Result<FileStamp, PdfError> {
    let bytes = compact(document, options, progress)?;
    Ok(write_bytes_atomic(path, expect, &bytes)?)
}

/// Whether the file had object streams or a cross-reference stream.
fn uses_object_streams(document: &Document) -> bool {
    document.file().sections().iter().any(|section| {
        section.form() == SectionForm::Stream
            || section.hybrid_offset().is_some()
            || section
                .entries()
                .any(|(_, entry)| matches!(entry, XrefEntry::Compressed { .. }))
    })
}

/// The trailer as written, before `Size` and the section's own entries
/// are added: the document's, less what describes the file's sections,
/// with a new second identifier.
fn written_trailer(
    document: &Document,
    trailer: &Dictionary,
    options: &WriteOptions,
) -> Dictionary {
    let mut written = Dictionary::new();
    for (key, value) in trailer.iter() {
        if !SECTION_KEYS.contains(&key.as_bytes()) {
            written.insert(key.clone(), value.clone());
        }
    }
    let changing = derive_id(&options.seed);
    let permanent = match trailer.get(ID_KEY).map(Object::value) {
        Some(Value::Array(items)) => match items.first().map(Object::value) {
            Some(Value::String(first)) => Some(first.clone()),
            _ => None,
        },
        _ => None,
    };
    let encrypted = trailer.get(ENCRYPT_KEY).is_some() || document.decryptor().is_some();
    let id = match permanent {
        Some(first) => Some([first, changing]),
        // The key of revisions 2 to 4 is computed from the first
        // identifier, taken as empty when there is none (7.6.3.3): an
        // encrypted file without one is left without one.
        None if encrypted => None,
        None => Some([changing.clone(), changing]),
    };
    match id {
        Some([first, second]) => {
            written.insert(
                Name::new(ID_KEY),
                Object::new(Value::Array(vec![
                    Object::new(Value::String(first)),
                    Object::new(Value::String(second)),
                ])),
            );
        }
        None => {
            written.remove(ID_KEY);
        }
    }
    written.remove(SIZE_KEY);
    written
}

fn derive_id(seed: &[u8; SEED_BYTES]) -> PdfString {
    use sha2::Digest;
    let mut hash = sha2::Sha256::new();
    hash.update(ID_LABEL);
    hash.update(seed);
    let digest = hash.finalize();
    PdfString::new(&digest[..ID_BYTES])
}

/// An object identifier of generation 0, for an object this writer makes.
fn new_id(number: u32) -> ObjectId {
    ObjectId {
        number,
        generation: 0,
    }
}
