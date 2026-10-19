// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Text files for an editor: decode bytes to text, remember the line ending
//! and byte order mark so a save puts them back, and write atomically.
//!
//! Specifications: the Unicode Standard, section 23.8 (Specials), for the
//! byte order mark U+FEFF, which UTF-8 encodes as `EF BB BF` and UTF-16
//! writes as `FE FF` (big endian) or `FF FE` (little endian); RFC 3629
//! section 4 for what makes a byte sequence valid UTF-8.
//!
//! A file that cannot be saved back byte for byte (not UTF-8, UTF-16, mixed
//! line endings, too large to edit, not writable) is still opened, as a
//! lossy display copy marked [`ReadOnly`]; the caller must not offer Save
//! for it. [`save`] itself never checks that mark.

mod decode;
mod encode;
mod error;
mod open;

pub use decode::{decode, Decoded, LineEnding, ReadOnly};
pub use encode::encode;
pub use error::TextError;
pub use open::{open, save, TextFile, EDITABLE_LIMIT, OPEN_LIMIT};
