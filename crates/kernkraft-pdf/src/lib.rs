// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! PDF reading, atomic saving and page operations over `lopdf`, with page
//! rendering through MuPDF behind the `render` feature.
//!
//! Page numbers in every API are 1-based. Documents are edited in memory as
//! [`lopdf::Document`] and written back through `kernkraft-fs`'s atomic
//! writer. Every operation that changes the page tree first flattens it
//! (see [`flatten_page_tree`]), so pages keep the attributes they inherited
//! from intermediate page-tree nodes.

mod doc;
mod error;
mod fields;
mod merge;
mod pages;
mod ranges;
#[cfg(feature = "render")]
mod render;
mod split;
mod tree;

pub use lopdf;

pub use doc::{load, load_bytes, page_count, save, to_bytes};
pub use error::PdfError;
pub use kernkraft_fs::FileStamp;
pub use merge::merge;
pub use pages::{remove, reorder, rotate, Rotation};
pub use ranges::{parse_ranges, PageRange};
#[cfg(feature = "render")]
pub use render::{page_size_pt, render_page, RenderedPage, Renderer, MAX_RENDER_DIMENSION};
pub use split::{extract, split_by_bookmarks, split_per_page, split_ranges, START_PART_TITLE};
pub use tree::flatten_page_tree;
