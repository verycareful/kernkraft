// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! An edit session: ISO 32000-1:2008, 7.5.6 (incremental updates), 14.3.3
//! (document information: `ModDate`), 14.4 (file identifiers).
//!
//! A session holds a [`Document`], the snapshots that edits made of it, and
//! where it stands among them, so that an edit can be undone and done
//! again. The bytes of the file it was opened from are never changed: a
//! save writes them as they are, followed by one update with everything
//! that differs from them ([`EditSession::save`]), or returns that update
//! for an application that can only append to the file
//! ([`EditSession::append_since`]).
//!
//! Undo and redo move between snapshots. They write nothing; the bytes of
//! an update are made from the snapshot to be saved, so a save after an
//! undo holds exactly what the document was then, whatever was saved
//! before.
//!
//! A file whose cross-reference was rebuilt, and an earlier revision of a
//! file, are not edited in place ([`PdfError::RebuiltNotEditable`],
//! [`PdfError::RevisionNotEditable`]): the application saves them as a
//! compacted copy ([`write::save_compact`](crate::write::save_compact)).

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::Path;

use kernkraft_fs::{write_atomic, Expect, FileStamp, Progress};

use crate::doc::{Changes, Date, Document, ObjectBuilder};
use crate::syntax::{Dictionary, Name, Object, PdfString, Value};
use crate::write::{incremental, Tail, Update, WriteOptions};
use crate::{PdfError, Source};

const INFO_KEY: &[u8] = b"Info";
const MOD_DATE_KEY: &[u8] = b"ModDate";

/// How many bytes of the file a save copies at a time.
const COPY_CHUNK: u64 = 1 << 20;

/// What a save takes from the caller, so that the same edits give the same
/// bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaveOptions {
    /// The seed of the second file identifier and of the initialization
    /// vectors of AES; fresh for each save.
    pub write: WriteOptions,
    /// The time of the save, which becomes the document's `ModDate`.
    pub modified: Date,
}

/// An update returned for the caller to append to its copy of the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Appended {
    /// The bytes to append: nothing when the document is what the file
    /// already holds.
    pub bytes: Vec<u8>,
    /// How long the file is once they are appended. The next
    /// [`EditSession::append_since`] is given this length, once
    /// [`EditSession::confirm_appended`] has been told the bytes are
    /// written.
    pub length: u64,
}

/// An append the session made and has not been told was written, or was.
struct Point {
    tail: Tail,
    /// The document the file holds once the append is written.
    document: Document,
}

/// A document being edited. See the [module](self).
pub struct EditSession {
    /// The snapshots, the document as given first.
    snapshots: Vec<Document>,
    /// The snapshot the session stands on.
    cursor: usize,
    /// The file as opened, which every save starts from.
    base: Tail,
    /// What the file on disk holds, in the terms of the document: the
    /// session is modified when the current snapshot is not it.
    saved: Document,
    /// The appends made, by the length the file has once each is written.
    points: BTreeMap<u64, Point>,
}

impl std::fmt::Debug for EditSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EditSession")
            .field("snapshots", &self.snapshots.len())
            .field("cursor", &self.cursor)
            .field("modified", &self.is_modified())
            .finish_non_exhaustive()
    }
}

impl EditSession {
    /// A session editing `document`. Edits `document` carries already
    /// count as unsaved: the session is modified from the start.
    ///
    /// A document whose cross-reference was rebuilt is
    /// [`PdfError::RebuiltNotEditable`] and an earlier revision of a file
    /// is [`PdfError::RevisionNotEditable`].
    pub fn new(document: Document) -> Result<Self, PdfError> {
        if document.is_revision_view() {
            return Err(PdfError::RevisionNotEditable);
        }
        let base = Tail::of_file(&document)?;
        let saved = document.unedited();
        Ok(Self {
            snapshots: vec![document],
            cursor: 0,
            base,
            saved,
            points: BTreeMap::new(),
        })
    }

    /// The document as it stands.
    pub fn document(&self) -> &Document {
        &self.snapshots[self.cursor]
    }

    /// Edits for the document as it stands, to be given to
    /// [`Self::apply`].
    pub fn builder(&self) -> ObjectBuilder {
        ObjectBuilder::new(self.document())
    }

    /// Applies `changes`, built for the document as it stands, and makes
    /// the result the document. What could be redone is dropped. Changes
    /// that change nothing add no step and give `false`.
    pub fn apply(&mut self, changes: Changes) -> Result<bool, PdfError> {
        if changes.is_empty() {
            return Ok(false);
        }
        let next = self.document().apply(changes)?;
        self.snapshots.truncate(self.cursor + 1);
        self.snapshots.push(next);
        self.cursor += 1;
        Ok(true)
    }

    /// Makes `document` the document as it stands: a snapshot of the same
    /// file made from the document as it stands, as a page operation
    /// ([`rotate`](crate::rotate), [`reorder`](crate::reorder),
    /// [`remove`](crate::remove)) gives one. What could be redone is
    /// dropped. A document that is the one as it stands adds no step and
    /// gives `false`; one of another file is [`PdfError::Invalid`], since
    /// an update to this file cannot hold it (a merged or extracted
    /// document is saved as a compacted copy).
    pub fn push(&mut self, document: Document) -> Result<bool, PdfError> {
        if !self.document().same_file(&document) {
            return Err(PdfError::Invalid(
                "the document is not a snapshot of the file being edited".to_string(),
            ));
        }
        if self.document().same_edits(&document) {
            return Ok(false);
        }
        self.snapshots.truncate(self.cursor + 1);
        self.snapshots.push(document);
        self.cursor += 1;
        Ok(true)
    }

    pub fn can_undo(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_redo(&self) -> bool {
        self.cursor + 1 < self.snapshots.len()
    }

    /// Goes back one step. `false` when there is none.
    pub fn undo(&mut self) -> bool {
        if !self.can_undo() {
            return false;
        }
        self.cursor -= 1;
        true
    }

    /// Goes forward one step undone. `false` when there is none.
    pub fn redo(&mut self) -> bool {
        if !self.can_redo() {
            return false;
        }
        self.cursor += 1;
        true
    }

    /// Whether the document differs from what the file holds: the file as
    /// opened until a save, and what the last save wrote after it. An edit
    /// that is undone leaves the session unmodified again.
    pub fn is_modified(&self) -> bool {
        !self.saved.same_edits(self.document())
    }

    /// Writes the file to `path`: the bytes of the file as opened,
    /// followed by one update with everything that differs from them. A
    /// document that does not differ is written as the file is. `expect`
    /// says what may be at `path`, as [`write_atomic`] has it; the write is
    /// atomic.
    ///
    /// The session goes on reading the bytes it opened, also when `path`
    /// is the file. A save counts as the state the file holds from then on
    /// ([`Self::is_modified`]).
    pub fn save(
        &mut self,
        path: &Path,
        expect: Expect<'_>,
        options: &SaveOptions,
        progress: &dyn Progress,
    ) -> Result<FileStamp, PdfError> {
        let document = self.document().clone();
        let update = update_from(&self.base, &document, options, progress)?;
        let appended = update.as_ref().map_or(&[][..], |update| &update.bytes);
        let source = document.file().source().clone();
        let mut failure = None;
        let written = write_atomic(path, expect, |out| {
            copy_source(&*source, out, progress, &mut failure)?;
            out.write_all(appended)
        });
        if let Some(error) = failure {
            return Err(error);
        }
        let stamp = written?;
        self.saved = document;
        Ok(stamp)
    }

    /// The update to append to a copy of the file that is `length` bytes
    /// long: the length of the file as opened, or one an earlier call
    /// returned and [`Self::confirm_appended`] was told was written. For an
    /// application that cannot replace the file and appends to it through a
    /// document provider.
    ///
    /// The bytes turn the file as it stands there into the document as it
    /// stands here, so an edit that was appended and then undone is undone
    /// by them. A length the session did not leave the file at is
    /// [`PdfError::UnknownAppendPoint`].
    pub fn append_since(
        &mut self,
        length: u64,
        options: &SaveOptions,
        progress: &dyn Progress,
    ) -> Result<Appended, PdfError> {
        let from = if length == self.base.length() {
            &self.base
        } else {
            &self
                .points
                .get(&length)
                .ok_or(PdfError::UnknownAppendPoint { length })?
                .tail
        };
        let document = self.document().clone();
        let Some(update) = update_from(from, &document, options, progress)? else {
            return Ok(Appended {
                bytes: Vec::new(),
                length,
            });
        };
        let length = update.tail.length();
        self.points.insert(
            length,
            Point {
                tail: update.tail,
                document,
            },
        );
        Ok(Appended {
            bytes: update.bytes,
            length,
        })
    }

    /// Tells the session that the bytes [`Self::append_since`] returned
    /// for a file that is now `length` bytes long are written, so that the
    /// file holds that document ([`Self::is_modified`]).
    pub fn confirm_appended(&mut self, length: u64) -> Result<(), PdfError> {
        let point = self
            .points
            .get(&length)
            .ok_or(PdfError::UnknownAppendPoint { length })?;
        self.saved = point.document.clone();
        Ok(())
    }
}

/// The update that turns the file `from` holds into `document`, with the
/// time of the save as its `ModDate`; `None` when `document` is what the
/// file holds.
fn update_from(
    from: &Tail,
    document: &Document,
    options: &SaveOptions,
    progress: &dyn Progress,
) -> Result<Option<Update>, PdfError> {
    if !from.differs(document) {
        return Ok(None);
    }
    let stamped = with_modification_date(document, &options.modified)?;
    incremental(from, &stamped, &options.write, progress).map(Some)
}

/// `document` with `date` as the `ModDate` of its information dictionary
/// (14.3.3), which is made when the document has none.
fn with_modification_date(document: &Document, date: &Date) -> Result<Document, PdfError> {
    let mut info = document.info()?.unwrap_or_else(Dictionary::new);
    info.insert(
        Name::new(MOD_DATE_KEY),
        Object::new(Value::String(PdfString::new(date.format().as_bytes()))),
    );
    let info = Object::new(Value::Dictionary(info));
    let mut builder = ObjectBuilder::new(document);
    match document.trailer().get(INFO_KEY).map(Object::value) {
        Some(Value::Reference(id)) => builder.set(*id, info),
        _ => {
            let id = builder.add(info)?;
            builder.set_trailer(Name::new(INFO_KEY), Some(Object::new(Value::Reference(id))));
        }
    }
    document.apply(builder.finish())
}

/// Writes all of `source` to `out`, a chunk at a time. What fails in the
/// source is left in `failure`, since `out`'s error type is `io::Error`.
fn copy_source(
    source: &dyn Source,
    out: &mut dyn Write,
    progress: &dyn Progress,
    failure: &mut Option<PdfError>,
) -> io::Result<()> {
    let total = source.len();
    let mut done = 0;
    while done < total {
        if progress.is_cancelled() {
            *failure = Some(PdfError::Cancelled);
            return Err(io::Error::other("cancelled"));
        }
        progress.report(done, total, "copying");
        // At most COPY_CHUNK, which fits in a usize.
        let len = (total - done).min(COPY_CHUNK) as usize;
        match source.read(done, len) {
            Ok(chunk) => out.write_all(&chunk)?,
            Err(error) => {
                *failure = Some(error);
                return Err(io::Error::other("the source could not be read"));
            }
        }
        done += len as u64;
    }
    progress.report(total, total, "copying");
    Ok(())
}
