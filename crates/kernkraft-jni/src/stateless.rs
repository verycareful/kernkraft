// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The operations that keep no session, as request strings: merge, split by
//! ranges, extract and compact a file. Files are opened under [`OPEN_OPTIONS`](crate::open::OPEN_OPTIONS),
//! and a request may name a job to cancel it, like any long request.

use crate::bridge::bridge;

/// Concatenates the sources, in order, into the new file `out`.
pub fn merge_files(request: &str) -> String {
    bridge().merge_files(request)
}

/// The listed pages of the source as the new file `out`.
pub fn extract_pages(request: &str) -> String {
    bridge().extract_pages(request)
}

/// The source as the new, compacted file `out`.
pub fn compact_file(request: &str) -> String {
    bridge().compact_file(request)
}

/// One new file per range, in `out_dir`, named after the source.
pub fn split_by_ranges(request: &str) -> String {
    bridge().split_by_ranges(request)
}
