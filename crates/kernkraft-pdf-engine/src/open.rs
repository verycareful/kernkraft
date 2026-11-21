// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Opening a file, and the seed a save takes.

use std::path::PathBuf;

use kernkraft_pdf::doc::{Document, OpenOptions};
use kernkraft_pdf::write::WriteOptions;
use serde::Deserialize;

use crate::error::WireError;

/// A PDF file on the device's own storage and the password that opens it,
/// when it has one. The path is kept as the bytes the file system gave, so
/// a name that is not UTF-8 names its file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub path: PathBuf,
    #[serde(default)]
    pub password: String,
}

impl Source {
    /// Opens the file under `options`, the limits the adapter chose for
    /// its device.
    pub fn open(&self, options: &OpenOptions) -> Result<Document, WireError> {
        Ok(Document::open(&self.path, options, &self.password)?)
    }
}

/// The options of one save: a seed fresh from the operating system, so that
/// no two saves share a file identifier.
pub fn write_options() -> Result<WriteOptions, WireError> {
    let mut seed = [0_u8; 32];
    getrandom::fill(&mut seed)
        .map_err(|error| WireError::new("RandomUnavailable", error.to_string()))?;
    Ok(WriteOptions { seed })
}
