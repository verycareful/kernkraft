// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

use std::path::{Path, PathBuf};

/// Every failure `kernkraft-xdg-apps` reports.
#[derive(Debug, thiserror::Error)]
pub enum AppsError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// A line of a MIME database file that does not have the documented
    /// shape. `line` is 1-based.
    #[error("{path}:{line}: malformed MIME database line")]
    MalformedMimeDb { path: PathBuf, line: usize },
}

impl AppsError {
    /// An I/O failure at `path`.
    pub(crate) fn io(path: &Path, source: std::io::Error) -> AppsError {
        AppsError::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}
