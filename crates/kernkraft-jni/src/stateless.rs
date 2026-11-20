// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The operations that keep no session, as request strings: merge, split by
//! ranges and extract. Files are opened under [`OPEN_OPTIONS`].

use kernkraft_pdf_engine::stateless::{self, ExtractRequest, MergeRequest, SplitRequest};

use crate::guard::run;
use crate::open::OPEN_OPTIONS;

/// Concatenates the sources, in order, into the new file `out`.
pub fn merge_files(request: &str) -> String {
    run(request, |request: MergeRequest| {
        stateless::merge_files(&OPEN_OPTIONS, request)
    })
}

/// The listed pages of the source as the new file `out`.
pub fn extract_pages(request: &str) -> String {
    run(request, |request: ExtractRequest| {
        stateless::extract_pages(&OPEN_OPTIONS, request)
    })
}

/// One new file per range, in `out_dir`, named after the source.
pub fn split_by_ranges(request: &str) -> String {
    run(request, |request: SplitRequest| {
        stateless::split_by_ranges(&OPEN_OPTIONS, request)
    })
}
