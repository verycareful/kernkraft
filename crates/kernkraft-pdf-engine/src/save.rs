// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Getting the edits out. An application that cannot replace the file (Kern
//! Android, which writes through a document provider) takes the update to
//! append: `append_since` hands the new bytes over in a file of the
//! application's cache, and once the application has appended them it says
//! so with `confirm_appended`, and only then does the session count the
//! document as saved. An application that can ([`Engine::save`]) has the file
//! written for it, atomically. Either can write a compacted copy.

use std::path::{Path, PathBuf};

use kernkraft_fs::{write_bytes_atomic, Expect, FileStamp};
use kernkraft_pdf::write::save_compact;
use kernkraft_pdf::SaveOptions;
use serde::Deserialize;
use serde_json::json;

use crate::engine::Engine;
use crate::error::WireError;
use crate::fields::{field, Fields};
use crate::jobs::JobId;
use crate::open::write_options;
use crate::wire;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppendRequest {
    pub handle: i64,
    /// The length the file has now: the length it was opened with, or the
    /// `length` of the last append that was confirmed.
    pub base_length: u64,
    /// Where the bytes to append are written: a new file.
    pub out: PathBuf,
    /// The time of the save, as a PDF date.
    pub modified: String,
    /// The job that can cancel this request and watch it (see
    /// [`crate::jobs`]); `done` counts objects.
    #[serde(default)]
    pub job: Option<JobId>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmRequest {
    pub handle: i64,
    /// The `length` the append answered with.
    pub length: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactRequest {
    pub handle: i64,
    pub out: PathBuf,
    /// The job that can cancel this request and watch it (see
    /// [`crate::jobs`]); `done` counts objects.
    #[serde(default)]
    pub job: Option<JobId>,
}

impl Engine {
    /// Writes the bytes that take the file from `base_length` to the
    /// document as it stands into the new file `out`. No file is written
    /// when there is nothing to append (`appended` is 0).
    pub fn append_since(&self, request: AppendRequest) -> Result<Fields, WireError> {
        let options = SaveOptions {
            write: write_options()?,
            modified: wire::date(&request.modified)?,
        };
        let running = self.jobs.begin(request.job, Some(request.handle))?;
        self.sessions.with(request.handle, |session| {
            let appended = session
                .edit
                .append_since(request.base_length, &options, &running)?;
            if !appended.bytes.is_empty() {
                write_bytes_atomic(&request.out, Expect::Nothing, &appended.bytes)
                    .map_err(kernkraft_pdf::PdfError::from)?;
            }
            let mut fields = field("length", appended.length);
            fields.insert("appended".to_owned(), json!(appended.bytes.len()));
            Ok(fields)
        })
    }

    /// Writes the opened bytes and one update with every edit to `path`,
    /// atomically, as far as `expect` allows (see `kernkraft_fs::Expect`),
    /// and gives back the stamp of the file written. The session goes on
    /// reading the bytes it opened, so a save over the opened file is safe
    /// and the next save is made against the opened bytes again. `modified`
    /// is the time of the save as a PDF date. `job` names the job that can
    /// cancel the save and watch it (see [`crate::jobs`]); a save that is
    /// cancelled writes nothing.
    pub fn save(
        &self,
        handle: i64,
        path: &Path,
        expect: Expect<'_>,
        modified: &str,
        job: Option<JobId>,
    ) -> Result<FileStamp, WireError> {
        let options = SaveOptions {
            write: write_options()?,
            modified: wire::date(modified)?,
        };
        let running = self.jobs.begin(job, Some(handle))?;
        self.sessions.with(handle, |session| {
            Ok(session.edit.save(path, expect, &options, &running)?)
        })
    }

    /// Tells the session the bytes of an append are written to the file.
    pub fn confirm_appended(&self, request: ConfirmRequest) -> Result<Fields, WireError> {
        self.sessions.with(request.handle, |session| {
            session.edit.confirm_appended(request.length)?;
            Ok(field("modified", session.edit.is_modified()))
        })
    }

    /// The document as it stands, written as a new, compacted file `out`;
    /// the original is untouched. Gives back `out`.
    pub fn compact(&self, request: CompactRequest) -> Result<PathBuf, WireError> {
        let running = self.jobs.begin(request.job, Some(request.handle))?;
        let (document, _) = self.snapshot(request.handle)?;
        save_compact(
            &document,
            &request.out,
            Expect::Nothing,
            &write_options()?,
            &running,
        )
        .map_err(WireError::from)?;
        Ok(request.out)
    }
}
