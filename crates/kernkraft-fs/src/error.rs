// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

use std::path::PathBuf;

/// Every failure `kernkraft-fs` reports.
#[derive(Debug, thiserror::Error)]
pub enum FsError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} changed on disk since it was opened")]
    ExternalModification { path: PathBuf },
    #[error("{path} already exists")]
    Exists { path: PathBuf },
    #[error("{path} not found")]
    NotFound { path: PathBuf },
    #[error("invalid name: {name:?}")]
    InvalidName { name: String },
    #[error("cannot place {src} inside itself")]
    IntoItself { src: PathBuf },
    #[error("no usable trash for {path}")]
    TrashUnavailable { path: PathBuf },
    #[error("malformed trash info {path}")]
    BadTrashInfo { path: PathBuf },
    #[error("unsafe cache directory {path}")]
    UnsafeCacheDir { path: PathBuf },
    #[error("cancelled")]
    Cancelled,
}
