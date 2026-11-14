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
//! it and decrypts the file's strings and streams. [`doc`] reads a
//! document's objects as they are used, its pages and its trees, and
//! gives a new snapshot for each edit; [`build`] makes the objects and
//! content streams of an edit; [`write`] writes a document as a new file
//! that keeps every untouched object as its original bytes. A
//! [`Document`](doc::Document) also gives its revisions and an earlier
//! one opened, its signatures and what their byte ranges cover, a report
//! of what in it could run or reach outside it, its page labels, its
//! outline and its attachments.

mod action;
mod active;
mod attachments;
pub mod build;
pub mod crypt;
mod destination;
mod diagnostic;
pub mod doc;
mod error;
mod fields;
pub mod file;
mod filespec;
pub mod filter;
mod labels;
mod lopdf_io;
mod merge;
mod outline;
mod pages;
mod pdf_doc;
mod ranges;
#[cfg(feature = "render")]
mod render;
mod revisions;
mod signatures;
mod source;
mod split;
pub mod syntax;
mod tree;
pub mod write;

pub use lopdf;

pub use action::{Action, ActionKind};
pub use active::{ActiveAction, ActiveContent};
pub use attachments::{Attachment, AttachmentPlace, Check, Extracted};
pub use destination::{Destination, DestinationPage, View};
pub use diagnostic::{Diagnostic, DiagnosticKind, Diagnostics, PortableLimit};
pub use error::{Limit, PdfError};
pub use filespec::FileSpec;
pub use kernkraft_fs::FileStamp;
pub use labels::{LabelStyle, PageLabel};
pub use lopdf_io::{load, load_bytes, page_count, save, to_bytes};
pub use merge::merge;
pub use outline::OutlineItem;
pub use pages::{remove, reorder, rotate, Rotation};
pub use ranges::{parse_ranges, PageRange};
#[cfg(feature = "render")]
pub use render::{page_size_pt, render_page, RenderedPage, Renderer, MAX_RENDER_DIMENSION};
pub use revisions::Revision;
pub use signatures::{Coverage, Signature, SignatureKind};
pub use source::{FileSource, MemorySource, Source};
pub use split::{extract, split_by_bookmarks, split_per_page, split_ranges, START_PART_TITLE};
pub use tree::flatten_page_tree;
