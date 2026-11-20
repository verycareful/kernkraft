// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Getting the edits out: the update to append to the opened file, and a
//! compacted copy.
//!
//! The application writes the original through a document provider, so the
//! session never writes it. `appendSince` hands the new bytes over in a file
//! of the application's cache; once the application has appended them it
//! says so with `confirmAppended`, and only then does the session count the
//! document as saved.

use std::path::Path;

use kernkraft_fs::{write_bytes_atomic, Expect, NoProgress};
use kernkraft_pdf::write::save_compact;
use kernkraft_pdf::SaveOptions;
use serde::Deserialize;
use serde_json::json;

use crate::engine::Engine;
use crate::error::WireError;
use crate::fields::{field, Fields};
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
    pub out: String,
    /// The time of the save, as a PDF date.
    pub modified: String,
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
    pub out: String,
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
        self.sessions.with(request.handle, |session| {
            let appended = session
                .edit
                .append_since(request.base_length, &options, &NoProgress)?;
            if !appended.bytes.is_empty() {
                write_bytes_atomic(Path::new(&request.out), Expect::Nothing, &appended.bytes)
                    .map_err(kernkraft_pdf::PdfError::from)?;
            }
            let mut fields = field("length", appended.length);
            fields.insert("appended".to_owned(), json!(appended.bytes.len()));
            Ok(fields)
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
    /// the original is untouched.
    pub fn compact(&self, request: CompactRequest) -> Result<Fields, WireError> {
        let (document, _) = self.snapshot(request.handle)?;
        save_compact(
            &document,
            Path::new(&request.out),
            Expect::Nothing,
            &write_options()?,
            &NoProgress,
        )
        .map_err(WireError::from)?;
        Ok(field("path", request.out))
    }
}
