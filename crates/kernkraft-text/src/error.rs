// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

use std::path::PathBuf;

use kernkraft_fs::FsError;

use crate::ReadOnly;

/// Every failure `kernkraft-text` reports.
#[derive(Debug, thiserror::Error)]
pub enum TextError {
    #[error(transparent)]
    Fs(#[from] FsError),
    #[error("file is {len} bytes, above the {limit} byte limit for opening")]
    TooLargeToOpen { len: u64, limit: u64 },
    /// A save would replace a file it must not: one whose content cannot be
    /// saved back byte for byte (not UTF-8, UTF-16, mixed line endings), or
    /// one the user has no write permission for
    /// ([`ReadOnly::NoWritePermission`]). Nothing was written.
    #[error("{}: not saved, the file on disk is {reason}", path.display())]
    ReadOnly { path: PathBuf, reason: ReadOnly },
}
