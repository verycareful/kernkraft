// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Paths on the IPC wire.
//!
//! A Unix path is a byte string, not text, but JSON carries only Unicode. A
//! path that is valid UTF-8 travels as a plain string; any other travels as
//! the base64 (RFC 4648 section 4, standard alphabet, padded) of its raw
//! bytes, tagged so the receiver can tell the two apart.
//!
//! The webview is untrusted. Deserialising a [`WirePath`] never fails: a
//! value of any other shape becomes [`WirePath::Invalid`], so a command
//! taking a path reports it as a [`WireError`](crate::WireError) of kind
//! `BadPath` instead of failing Tauri's argument parsing with a bare
//! string. Decoding then refuses a path that is not absolute (the empty
//! path included): a relative path would be resolved against the
//! process's working directory, which for a desktop app is usually the
//! home folder, and nothing the UI shows is relative.

use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};

/// A path as it crosses the IPC boundary.
///
/// JSON form: `{"k":"Utf8","v":"/home/a"}` or `{"k":"Bytes","v":"YfZi"}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "k", content = "v")]
pub enum WirePath {
    /// A path that is valid UTF-8.
    Utf8(String),
    /// Any other path: the standard base64 of its bytes.
    Bytes(String),
    /// A received value that is neither form, described for the error
    /// message. kernkraft never sends one; decoding it fails with
    /// [`WirePathError::BadPath`].
    Invalid(String),
}

impl<'de> Deserialize<'de> for WirePath {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<WirePath, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Tagged { k: String, v: String },
            Other(serde::de::IgnoredAny),
        }
        Ok(match Raw::deserialize(d)? {
            Raw::Tagged { k, v } => match k.as_str() {
                "Utf8" => WirePath::Utf8(v),
                "Bytes" => WirePath::Bytes(v),
                _ => WirePath::Invalid(format!("unknown path form {k:?}")),
            },
            Raw::Other(_) => {
                WirePath::Invalid("not an object with a string \"k\" and \"v\"".to_owned())
            }
        })
    }
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
    /// The value received is not a path in either form.
    #[error("not a path: {0}")]
    BadPath(String),
    /// The path is empty or relative.
    #[error("path is not absolute: {0:?}")]
    NotAbsolute(String),
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
            WirePath::Invalid(what) => what.clone(),
        }
    }

    /// Decodes to a path. The webview is untrusted, so a malformed `Bytes`
    /// payload, an embedded NUL, a value of neither form, or a path that is
    /// not absolute is an error, never a panic or a silently different path.
    pub fn to_path_buf(&self) -> Result<PathBuf, WirePathError> {
        let path = match self {
            WirePath::Utf8(s) => PathBuf::from(s),
            WirePath::Bytes(b64) => {
                let bytes = STANDARD
                    .decode(b64)
                    .map_err(|e| WirePathError::BadBase64(e.to_string()))?;
                PathBuf::from(OsString::from_vec(bytes))
            }
            WirePath::Invalid(what) => return Err(WirePathError::BadPath(what.clone())),
        };
        if path.as_os_str().as_bytes().contains(&0) {
            return Err(WirePathError::Nul);
        }
        if !path.is_absolute() {
            return Err(WirePathError::NotAbsolute(
                path.to_string_lossy().into_owned(),
            ));
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
