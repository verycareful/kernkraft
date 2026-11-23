// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Tests of text editing (`src/edit_text/`). Files are written by
//! `pdf_fixture`, byte by byte; each test's expectations come from how its
//! content is built and from ISO 32000-1 (9.4), never from a run.
//!
//! The text is set in Courier, 600 units a glyph, so at 10 points each
//! glyph advances 6 points.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

#[path = "../pdf_fixture/mod.rs"]
mod pdf_fixture;

mod bidi;
mod edit;
mod editability;
mod form;
mod remove;
mod rotated;
mod session;

use kernkraft_fs::NoProgress;
use kernkraft_pdf::text::TextPage;

pub use pdf_fixture::*;

/// The page object and its first content stream.
pub const PAGE: u32 = 10;
pub const CONTENT: u32 = 11;
pub const COURIER: &[u8] =
    b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier /Encoding /WinAnsiEncoding >>";

/// A one-page document whose font `F1` is Courier. `fonts` is added to the
/// font resources, `entries` to the page, `contents` is the page's
/// `Contents` entry (a reference to object 11 when `None`), and `extra`
/// are objects beside; object 11 holds `content`.
pub fn build(
    content: &str,
    fonts: &str,
    entries: &str,
    contents: Option<&str>,
    extra: &[(u32, &[u8])],
) -> Fixture {
    let contents = contents.unwrap_or("11 0 R");
    let mut objects: Vec<(u32, Vec<u8>)> = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (
            2,
            format!(
                "<< /Type /Pages /Kids [{PAGE} 0 R] /Count 1 /MediaBox [0 0 612 792] \
                 /Resources << /Font << /F1 60 0 R {fonts} >> >> >>"
            )
            .into_bytes(),
        ),
        (3, b"<< /Producer (kernkraft tests) >>".to_vec()),
        (
            PAGE,
            format!("<< /Type /Page /Parent 2 0 R /Contents {contents} {entries} >>").into_bytes(),
        ),
        (CONTENT, stream("", content.as_bytes())),
        (60, COURIER.to_vec()),
    ];
    for (number, body) in extra {
        objects.push((*number, body.to_vec()));
    }
    table_file(&borrowed(&objects), "/Root 1 0 R /Info 3 0 R")
}

/// A one-page document with `content`.
pub fn one_page(content: &str) -> Fixture {
    build(content, "", "", None, &[])
}

pub fn text_of(fixture: &Fixture) -> TextPage {
    fixture
        .open()
        .text_page(1, &NoProgress)
        .expect("the text is read")
}
