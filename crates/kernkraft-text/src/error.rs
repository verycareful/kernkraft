// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

use kernkraft_fs::FsError;

/// Every failure `kernkraft-text` reports.
#[derive(Debug, thiserror::Error)]
pub enum TextError {
    #[error(transparent)]
    Fs(#[from] FsError),
    #[error("file is {len} bytes, above the {limit} byte limit for opening")]
    TooLargeToOpen { len: u64, limit: u64 },
}
