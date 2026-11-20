// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The application's fonts.

use std::path::PathBuf;

use kernkraft_font::FontList;
use kernkraft_fs::NoProgress;
use serde::Deserialize;
use serde_json::json;

use crate::engine::Engine;
use crate::error::WireError;
use crate::fields::{field, Fields};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FontsRequest {
    /// The font files of the system, as the application lists them. Their
    /// family, style and weight are read from the files.
    pub paths: Vec<PathBuf>,
}

impl Engine {
    /// Indexes the font files and keeps the list for every text edit and
    /// render after it. A file that is not a font is skipped and counted,
    /// not an error.
    pub fn set_system_fonts(&self, request: FontsRequest) -> Result<Fields, WireError> {
        let list = FontList::index(&request.paths, &NoProgress)
            .map_err(|error| WireError::from(&error))?;
        let mut fields = field("faces", list.faces().len());
        fields.insert("skipped".to_owned(), json!(list.skipped().len()));
        self.set_fonts(list);
        Ok(fields)
    }
}
