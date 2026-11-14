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
//!
//! [`Source`] gives random access to a file's bytes, held in memory
//! ([`MemorySource`]) or read from disk by offset ([`FileSource`]).
//! [`syntax`] reads PDF objects from a source and writes them, reporting
//! what it accepted against the format as [`Diagnostic`]s. [`filter`]
//! decodes stream data, never past the size the caller allows. [`file`]
//! reads a file's header and cross-reference and loads its objects one at
//! a time, within the limits the caller sets. [`crypt`] reads the
//! encryption dictionary of an encrypted file, checks a password against
//! it and decrypts the file's strings and streams.

pub mod crypt;
mod diagnostic;
mod doc;
mod error;
mod fields;
pub mod file;
pub mod filter;
mod merge;
mod pages;
mod ranges;
#[cfg(feature = "render")]
mod render;
mod source;
mod split;
pub mod syntax;
mod tree;

pub use lopdf;

pub use diagnostic::{Diagnostic, DiagnosticKind, Diagnostics, PortableLimit};
pub use doc::{load, load_bytes, page_count, save, to_bytes};
pub use error::{Limit, PdfError};
pub use kernkraft_fs::FileStamp;
pub use merge::merge;
pub use pages::{remove, reorder, rotate, Rotation};
pub use ranges::{parse_ranges, PageRange};
#[cfg(feature = "render")]
pub use render::{page_size_pt, render_page, RenderedPage, Renderer, MAX_RENDER_DIMENSION};
pub use source::{FileSource, MemorySource, Source};
pub use split::{extract, split_by_bookmarks, split_per_page, split_ranges, START_PART_TITLE};
pub use tree::flatten_page_tree;
