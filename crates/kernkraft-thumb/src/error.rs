// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

use std::path::PathBuf;

use kernkraft_fs::FsError;

/// Every failure `kernkraft-thumb` reports.
#[derive(Debug, thiserror::Error)]
pub enum ThumbError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: cannot decode: {reason}")]
    Decode { path: PathBuf, reason: String },
    #[error("no thumbnails for {mime}")]
    Unsupported { mime: String },
    #[error(transparent)]
    Fs(#[from] FsError),
}

impl ThumbError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        ThumbError::Io {
            path: path.into(),
            source,
        }
    }

    pub(crate) fn decode(path: impl Into<PathBuf>, reason: impl ToString) -> Self {
        ThumbError::Decode {
            path: path.into(),
            reason: reason.to_string(),
        }
    }
}
