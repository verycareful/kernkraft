// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Limits of the syntax layer.
//!
//! The `MAX_*` constants are hard: input that passes one is refused with
//! [`PdfError::LimitExceeded`](crate::PdfError::LimitExceeded). ISO 32000
//! sets no such limit, so each states its reason.
//!
//! The `PORTABLE_*` constants are the architectural limits of ISO
//! 32000-1:2008, Annex C, Table C.1: what a conforming reader on a 32-bit
//! machine should accommodate. A value past one is read as written and
//! reported as a diagnostic.

/// Arrays and dictionaries nested one inside another. The parser, the
/// serialiser and the dropping of an object all recurse once per level, so
/// this bounds their stack use. Deep structures in a PDF (page trees,
/// outlines, structure trees) nest through indirect references, not inside
/// one object.
pub const MAX_NESTING_DEPTH: usize = 256;

/// The bytes of one name, after its `#` escapes are decoded. Bounds what one
/// token allocates; the value is `u16::MAX`.
pub const MAX_NAME_BYTES: usize = u16::MAX as usize;

/// The bytes of one keyword or number. Bounds what one token allocates; the
/// value is `u16::MAX`.
pub const MAX_TOKEN_BYTES: usize = u16::MAX as usize;

/// The decoded bytes of one string: 64 MiB. Bounds what one token
/// allocates. The longest strings in practice are signature contents with
/// revocation data embedded, which stay well under it; bulk data belongs in
/// streams.
pub const MAX_STRING_BYTES: usize = 1 << 26;

/// The filters one stream may chain. Each filter is a pass over the whole
/// stream, so this bounds the work a `Filter` array can ask for. Streams in
/// practice chain at most three (an ASCII filter, a compression filter and
/// an image filter).
pub const MAX_FILTERS: usize = 16;

/// Table C.1, integer: the largest integer value.
pub const PORTABLE_INTEGER_MAX: i64 = i32::MAX as i64;

/// Table C.1, integer: the smallest integer value.
pub const PORTABLE_INTEGER_MIN: i64 = i32::MIN as i64;

/// Table C.1, real: the largest magnitude of a real value (approximate).
pub const PORTABLE_REAL_MAX: f64 = 3.403e38;

/// Table C.1, name: the maximum length of a name, in bytes.
pub const PORTABLE_NAME_BYTES: usize = 127;

/// Table C.1, indirect object: the maximum number of indirect objects in a
/// file, and so the largest portable object number.
pub const PORTABLE_OBJECT_COUNT: u32 = 8_388_607;
