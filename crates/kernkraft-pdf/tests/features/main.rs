// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Tests of the document features: one module for each source file
//! (`labels.rs`, `outline.rs`, `destination.rs`, `action.rs`,
//! `attachments.rs`, `active.rs`, `revisions.rs`, `signatures.rs`), and
//! a mutation test over them all. The
//! files are written by `pdf_fixture`, byte by byte, and each test's
//! expectations come from how its file is built.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

#[path = "../pdf_fixture/mod.rs"]
mod pdf_fixture;

mod action;
mod active;
mod attachments;
mod destination;
mod labels;
mod outline;
mod revisions;
mod robustness;
mod signatures;

use kernkraft_pdf::doc::Document;
use kernkraft_pdf::syntax::ObjectId;
use kernkraft_pdf::{Diagnostic, DiagnosticKind};

pub use pdf_fixture::*;

pub fn id(number: u32) -> ObjectId {
    ObjectId {
        number,
        generation: 0,
    }
}

/// A document of `pages` pages (`pages_document`) whose catalog, object
/// 1, has `catalog` added to its entries, with `extra` objects beside.
pub fn document_with(pages: u32, catalog: &str, extra: &[(u32, &[u8])]) -> Fixture {
    let mut objects = pages_document(pages);
    objects[0].1 = format!("<< /Type /Catalog /Pages 2 0 R {catalog} >>").into_bytes();
    for (number, body) in extra {
        objects.push((*number, body.to_vec()));
    }
    table_file(&borrowed(&objects), "/Root 1 0 R /Info 3 0 R")
}

/// The object of page `number`, counted from 1, in `pages_document`.
pub fn page_object(number: u32) -> u32 {
    10 + 2 * (number - 1)
}

/// The diagnostics of `document` of one kind.
pub fn of_kind(document: &Document, kind: &DiagnosticKind) -> Vec<Diagnostic> {
    document
        .diagnostics()
        .into_iter()
        .filter(|diagnostic| &diagnostic.kind == kind)
        .collect()
}
