// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Paths on the IPC wire.
//!
//! A Unix path is a byte string, not text, but JSON carries only Unicode. A
//! path that is valid UTF-8 travels as a plain string; any other travels as
//! the base64 (RFC 4648 section 4, standard alphabet, padded) of its raw
//! bytes, tagged so the receiver can tell the two apart.

use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};

/// A path as it crosses the IPC boundary.
///
/// JSON form: `{"k":"Utf8","v":"/home/a"}` or `{"k":"Bytes","v":"YfZi"}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "k", content = "v")]
pub enum WirePath {
    /// A path that is valid UTF-8.
    Utf8(String),
    /// Any other path: the standard base64 of its bytes.
    Bytes(String),
}

/// A [`WirePath`] that cannot be turned back into a path.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum WirePathError {
    /// The `Bytes` payload is not valid standard base64.
    #[error("path bytes are not valid base64: {0}")]
    BadBase64(String),
    /// The path holds a NUL byte, which no file name can contain.
    #[error("path contains a NUL byte")]
    Nul,
}

impl WirePath {
    /// The path for display. Bytes that are not UTF-8 are shown as U+FFFD,
    /// and a `Bytes` payload that does not decode is shown as its text.
    pub fn display(&self) -> String {
        match self {
            WirePath::Utf8(s) => s.clone(),
            WirePath::Bytes(b64) => match STANDARD.decode(b64) {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(_) => b64.clone(),
            },
        }
    }

    /// Decodes to a path. The webview is untrusted, so a malformed `Bytes`
    /// payload or an embedded NUL is an error, never a panic or a silently
    /// different path.
    pub fn to_path_buf(&self) -> Result<PathBuf, WirePathError> {
        let path = match self {
            WirePath::Utf8(s) => PathBuf::from(s),
            WirePath::Bytes(b64) => {
                let bytes = STANDARD
                    .decode(b64)
                    .map_err(|e| WirePathError::BadBase64(e.to_string()))?;
                PathBuf::from(OsString::from_vec(bytes))
            }
        };
        if path.as_os_str().as_bytes().contains(&0) {
            return Err(WirePathError::Nul);
        }
        Ok(path)
    }
}

impl From<&Path> for WirePath {
    fn from(path: &Path) -> WirePath {
        match path.to_str() {
            Some(s) => WirePath::Utf8(s.to_owned()),
            None => WirePath::Bytes(STANDARD.encode(path.as_os_str().as_bytes())),
        }
    }
}

impl From<&PathBuf> for WirePath {
    fn from(path: &PathBuf) -> WirePath {
        WirePath::from(path.as_path())
    }
}

impl TryFrom<WirePath> for PathBuf {
    type Error = WirePathError;

    fn try_from(wire: WirePath) -> Result<PathBuf, WirePathError> {
        wire.to_path_buf()
    }
}
