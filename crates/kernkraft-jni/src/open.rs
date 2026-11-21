// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! What this adapter chooses for opening a document: the limits
//! `kernkraft-pdf` leaves to the application.

use kernkraft_pdf::doc::OpenOptions;
use kernkraft_pdf::file::ReadLimits;

/// Array items and dictionary entries in one object. A phone holds an
/// object of this size beside the page it draws; a file past it is refused
/// with `LimitExceeded`.
const MAX_OBJECT_ELEMENTS: usize = 1 << 20;
/// Cross-reference entries over all sections of a file: 16 million objects.
const MAX_XREF_ENTRIES: usize = 16 << 20;
/// The decoded bytes of one object stream.
const MAX_OBJECT_STREAM_BYTES: usize = 64 << 20;
/// Objects kept in memory once read; others are read again when used.
const CACHED_OBJECTS: usize = 2048;
/// Decoded object streams kept in memory.
const CACHED_OBJECT_STREAMS: usize = 8;
/// Diagnostics kept; past it they are counted.
const MAX_DIAGNOSTICS: usize = 10_000;

pub const OPEN_OPTIONS: OpenOptions = OpenOptions {
    limits: ReadLimits {
        max_object_elements: MAX_OBJECT_ELEMENTS,
        max_xref_entries: MAX_XREF_ENTRIES,
        max_object_stream_bytes: MAX_OBJECT_STREAM_BYTES,
    },
    cached_objects: CACHED_OBJECTS,
    cached_object_streams: CACHED_OBJECT_STREAMS,
    max_diagnostics: MAX_DIAGNOSTICS,
};
