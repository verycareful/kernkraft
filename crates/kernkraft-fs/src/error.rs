// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

use std::path::{Path, PathBuf};

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
    #[error("{path} belongs to another user or group, which saving it would change")]
    OwnershipNotKept { path: PathBuf },
    #[error("{path}: extended attributes not kept: {}", names.join(", "))]
    XattrsNotKept { path: PathBuf, names: Vec<String> },
    #[error("cancelled")]
    Cancelled,
}

impl FsError {
    /// An I/O failure at `path`.
    pub(crate) fn io(path: &Path, source: std::io::Error) -> FsError {
        FsError::Io {
            path: path.to_path_buf(),
            source,
        }
    }

    /// [`FsError::NotFound`] when `source` says the path is missing, else
    /// [`FsError::io`].
    pub(crate) fn not_found_or_io(path: &Path, source: std::io::Error) -> FsError {
        if source.kind() == std::io::ErrorKind::NotFound {
            FsError::NotFound {
                path: path.to_path_buf(),
            }
        } else {
            FsError::io(path, source)
        }
    }
}
