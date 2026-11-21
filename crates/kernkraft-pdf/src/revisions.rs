// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The revisions of a file: ISO 32000-1:2008, 7.5.6 (incremental updates)
//! and Annex F.3.4 and F.3.11 (the two sections of a linearized file).
//!
//! Each incremental update appends a cross-reference section, so a file's
//! revisions are its sections, oldest first: the file as first written,
//! then each update. A linearized file is the exception. Its first-page
//! section lies near the start of the file and its main section at the
//! end, and the first-page section, though read as an update of the main
//! one (F.3.4), was written with it: the two are one revision. They are
//! taken as one when the first object of the file is a linearization
//! parameter dictionary (F.2) and the newer of the two oldest sections
//! lies before the older one in the file, which no update can do.
//!
//! A revision's length is where the file ended once the revision was
//! written, which is what a signature's byte range is measured against.

use std::ops::Range;
use std::sync::Arc;

use crate::crypt::decrypt_file;
use crate::diagnostic::Diagnostics;
use crate::doc::Document;
use crate::file::{Body, PdfFile, XrefEntry};
use crate::syntax::{ObjectId, Value};
use crate::PdfError;

const LINEARIZED_KEY: &[u8] = b"Linearized";
const ENCRYPT_KEY: &[u8] = b"Encrypt";

/// One revision of the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revision {
    /// 0 for the file as first written, one more for each update after.
    pub index: usize,
    /// The sections of [`PdfFile::sections`] the revision added, which
    /// counts the newest first: one, or two for a linearized file.
    pub sections: Range<usize>,
    /// The bytes of the file once the revision was written, from its
    /// first byte; `None` when the cross-reference was rebuilt or no
    /// `startxref` closing the revision was found.
    pub length: Option<u64>,
    /// Whether the revision is the two sections of a linearized file.
    pub linearized: bool,
}

impl Document {
    /// The revisions of the file, oldest first. A file whose
    /// cross-reference was rebuilt has one.
    pub fn revisions(&self) -> Result<Vec<Revision>, PdfError> {
        let file = self.file();
        let count = file.sections().len();
        let joined = count >= 2 && is_linearized(file)?;
        let mut newest_first = Vec::new();
        let mut start = 0;
        while start < count {
            let linearized = joined && start + 2 == count;
            let end = if linearized { count } else { start + 1 };
            newest_first.push((start..end, linearized));
            start = end;
        }
        let total = newest_first.len();
        let mut revisions = Vec::with_capacity(total);
        for (place, (sections, linearized)) in newest_first.into_iter().enumerate() {
            revisions.push(Revision {
                index: total - 1 - place,
                length: file.update_end(sections.start)?,
                sections,
                linearized,
            });
        }
        revisions.reverse();
        Ok(revisions)
    }

    /// The document as revision `index` of [`Self::revisions`] left it,
    /// with no edits. An encrypted file keeps the key it was opened with.
    ///
    /// An index the file does not have is [`PdfError::Invalid`]; a revision
    /// whose trailer names other encryption than the newest one's is
    /// [`PdfError::RevisionEncryption`].
    pub fn open_revision(&self, index: usize) -> Result<Self, PdfError> {
        let revisions = self.revisions()?;
        let revision = revisions
            .get(index)
            .ok_or_else(|| PdfError::Invalid(format!("the file has no revision {index}")))?;
        let file = self
            .file()
            .as_of(revision.sections.start)
            .ok_or_else(|| PdfError::Invalid(format!("the file has no revision {index}")))?;
        if file.trailer().get(ENCRYPT_KEY) != self.file().trailer().get(ENCRYPT_KEY) {
            return Err(PdfError::RevisionEncryption { revision: index });
        }
        let mut diagnostics = Diagnostics::new();
        let decryptor = self
            .decryptor()
            .map(|decryptor| Arc::new(decryptor.reopened()));
        let file = match &decryptor {
            Some(decryptor) => decrypt_file(file, decryptor.clone(), &mut diagnostics)?,
            None => file,
        };
        Ok(Self::revision_view(
            file,
            decryptor,
            self.options(),
            diagnostics,
        ))
    }
}

/// Whether the first object of `file` is a linearization parameter
/// dictionary (Annex F.2: a dictionary with `Linearized`) and the newer
/// of its two oldest sections lies before the older one.
fn is_linearized(file: &PdfFile) -> Result<bool, PdfError> {
    let sections = file.sections();
    let [.., first_page, main] = sections else {
        return Ok(false);
    };
    if first_page.offset() >= main.offset() {
        return Ok(false);
    }
    let first = sections
        .iter()
        .flat_map(|section| section.entries())
        .filter_map(|(number, entry)| match entry {
            XrefEntry::InUse { offset, generation } => Some((offset, number, generation)),
            _ => None,
        })
        .min();
    let Some((_, number, generation)) = first else {
        return Ok(false);
    };
    let id = ObjectId { number, generation };
    let object = match file.load_as_of(sections.len() - 2, id, &mut Diagnostics::new()) {
        Ok(object) => object,
        Err(error) if crate::file::is_damage(&error) => return Ok(false),
        Err(error) => return Err(error),
    };
    let Some(Body::Object(object)) = object.map(|object| object.body) else {
        return Ok(false);
    };
    let Value::Dictionary(dictionary) = object.value() else {
        return Ok(false);
    };
    Ok(dictionary
        .get(LINEARIZED_KEY)
        .is_some_and(|value| !matches!(value.value(), Value::Null)))
}
