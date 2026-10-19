// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Image thumbnails and the freedesktop thumbnail cache.
//!
//! [`lookup`] finds a cached thumbnail and [`generate`] makes one, both in
//! the shared cache that Dolphin, Thunar and other file managers read, as the
//! Thumbnail Managing Standard 0.9.0 lays it out. Raster images (PNG, JPEG,
//! WebP, GIF) go through the `image` crate, SVG through `resvg` with every
//! external resource refused. Thumbnails are written as PNG 1.2 (ISO 15948)
//! with the `tEXt` attributes the standard requires.
//!
//! Every public function is synchronous and reports failure as
//! [`ThumbError`]. A file that cannot be decoded never panics; it gets a
//! failure entry so it is not retried, and [`ThumbError::Decode`].

mod cache;
mod error;
mod render;

pub use cache::{file_uri, lookup, thumb_path, Lookup, ThumbSize, FAIL_DIR};
pub use error::ThumbError;
pub use render::{can_thumbnail, generate};
