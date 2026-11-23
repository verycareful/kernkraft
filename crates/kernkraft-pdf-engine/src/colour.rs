// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The application's CMYK profile.

use std::path::PathBuf;

use kernkraft_pdf::colour::{CmykProfile, CmykSource};
use serde::Deserialize;
use serde_json::json;

use crate::engine::Engine;
use crate::error::WireError;
use crate::fields::{field, Fields};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CmykProfileRequest {
    /// An ICC profile file for a CMYK colour space; none takes the
    /// application's profile away, and the bundled one is used again.
    #[serde(default)]
    pub path: Option<PathBuf>,
}

impl Engine {
    /// Reads the profile and keeps it for every document opened after,
    /// where the file has no output intent of its own. A document already
    /// open keeps the profile it was opened with. A profile that cannot be
    /// read or used is an error and the profile in force stays. The answer
    /// has `source`: `application`, or `bundled` after a request with no
    /// path.
    pub fn set_cmyk_profile(&self, request: CmykProfileRequest) -> Result<Fields, WireError> {
        let profile = request
            .path
            .map(|path| CmykProfile::from_file(&path))
            .transpose()?;
        let source = match &profile {
            Some(profile) => profile.source(),
            None => CmykSource::Bundled,
        };
        self.set_cmyk(profile);
        Ok(field(
            "source",
            json!(match source {
                CmykSource::Bundled => "bundled",
                CmykSource::OutputIntent | CmykSource::Application => "application",
            }),
        ))
    }
}
