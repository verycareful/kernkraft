// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

/// Every failure `kernkraft-pdf` reports.
#[derive(Debug, thiserror::Error)]
pub enum PdfError {
    #[error("not a readable PDF: {0}")]
    Parse(String),
    #[error("the PDF is encrypted")]
    Encrypted,
    #[error("the PDF has no pages")]
    NoPages,
    #[error("page {page} is out of range 1-{count}")]
    PageOutOfRange { page: u32, count: u32 },
    #[error("page {0} is listed twice")]
    DuplicatePage(u32),
    #[error("bad page range {0:?}")]
    BadRange(String),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Fs(#[from] kernkraft_fs::FsError),
    #[error("render failed: {0}")]
    Render(String),
}
