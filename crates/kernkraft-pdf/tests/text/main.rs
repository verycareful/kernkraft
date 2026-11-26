// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Tests of the text layer, search, links and destinations (`src/text/`,
//! `src/links.rs`, `src/destination.rs`). Files are written by
//! `pdf_fixture`, byte by byte, and each test's expectations come from how
//! its content is built and from the arithmetic of ISO 32000-1 (9.4.4),
//! never from a run.
//!
//! The text is set in Courier, whose glyphs are 600 units wide (the Core 14
//! metrics of 9.6.2.2), so at 10 points each advances 6 points. With no
//! font descriptor a glyph's box reaches 8 points above its baseline and 2
//! below (0.8 and 0.2 em, the constants of `text/collect.rs`). Pages are
//! 612 by 792 points, and geometry is checked as fractions of that.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

#[path = "../pdf_fixture/mod.rs"]
mod pdf_fixture;

mod bidi;
mod layout;
mod links;
mod robustness;
mod search;
mod sizeless;
mod structure;
mod wrapped;

use kernkraft_fs::NoProgress;
use kernkraft_pdf::doc::Document;
use kernkraft_pdf::text::TextPage;
use kernkraft_pdf::Rect;

pub use pdf_fixture::*;

pub fn id(number: u32) -> kernkraft_pdf::syntax::ObjectId {
    kernkraft_pdf::syntax::ObjectId {
        number,
        generation: 0,
    }
}

pub const PAGE_WIDTH: f64 = 612.0;
pub const PAGE_HEIGHT: f64 = 792.0;

/// Courier (600 units for every glyph) and the same font with code 1 named
/// `fi` (the ligature U+FB01), code 2 named `zzz`, a name no list maps to
/// text, and codes 3 to 5 named `mapsto`, `parenleftbig` and `braceex`, names
/// of the TeX fonts.
pub const FONT_OBJECTS: [(u32, &[u8]); 2] = [
    (
        60,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier /Encoding /WinAnsiEncoding >>",
    ),
    (
        61,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier \
          /Encoding << /BaseEncoding /WinAnsiEncoding /Differences [1 /fi 2 /zzz 3 /mapsto 4 /parenleftbig 5 /braceex] >> >>",
    ),
];

/// One page of a test document: its content and the entries added to its
/// dictionary.
#[derive(Clone, Copy)]
pub struct PageSpec<'a> {
    pub content: &'a str,
    pub entries: &'a str,
}

impl<'a> PageSpec<'a> {
    pub fn of(content: &'a str) -> Self {
        Self {
            content,
            entries: "",
        }
    }
}

/// The object number of page `index`, counted from 0, and of its content.
pub fn page_object(index: u32) -> u32 {
    10 + 2 * index
}

/// A document of `pages` pages that share the resources `F1` and `F2` and
/// `resources` besides, whose catalog has `catalog` added to its entries,
/// with `extra` objects beside.
pub fn document(
    pages: &[PageSpec<'_>],
    catalog: &str,
    resources: &str,
    extra: &[(u32, &[u8])],
) -> Fixture {
    let kids: Vec<String> = (0..pages.len() as u32)
        .map(|i| format!("{} 0 R", page_object(i)))
        .collect();
    let mut objects: Vec<(u32, Vec<u8>)> = vec![
        (
            1,
            format!("<< /Type /Catalog /Pages 2 0 R {catalog} >>").into_bytes(),
        ),
        (
            2,
            format!(
                "<< /Type /Pages /Kids [{}] /Count {} /MediaBox [0 0 612 792] \
                 /Resources << /Font << /F1 60 0 R /F2 61 0 R >> {resources} >> >>",
                kids.join(" "),
                pages.len()
            )
            .into_bytes(),
        ),
        (3, b"<< /Producer (kernkraft tests) >>".to_vec()),
    ];
    for (index, page) in pages.iter().enumerate() {
        let number = page_object(index as u32);
        objects.push((
            number,
            format!(
                "<< /Type /Page /Parent 2 0 R /Contents {} 0 R {} >>",
                number + 1,
                page.entries
            )
            .into_bytes(),
        ));
        objects.push((number + 1, stream("", page.content.as_bytes())));
    }
    for (number, body) in FONT_OBJECTS.iter().chain(extra) {
        objects.push((*number, body.to_vec()));
    }
    table_file(&borrowed(&objects), "/Root 1 0 R /Info 3 0 R")
}

/// A one-page document.
pub fn one_page(content: &str) -> Fixture {
    document(&[PageSpec::of(content)], "", "", &[])
}

/// The text of page 1 of `fixture`.
pub fn text_of(fixture: &Fixture) -> TextPage {
    fixture
        .open()
        .text_page(1, &NoProgress)
        .expect("the text is read")
}

/// The text layer of a one-page document with `content`.
pub fn page(content: &str) -> TextPage {
    text_of(&one_page(content))
}

pub fn assert_close(actual: f64, expected: f64, what: &str) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "{what}: {actual} is not {expected}"
    );
}

/// `rect` is the box given in points of the displayed page, measured from
/// its top left corner, as fractions of the page.
pub fn assert_rect(rect: Rect, left: f64, top: f64, right: f64, bottom: f64, what: &str) {
    assert_close(rect.left, left / PAGE_WIDTH, &format!("{what}: left"));
    assert_close(rect.top, top / PAGE_HEIGHT, &format!("{what}: top"));
    assert_close(rect.right, right / PAGE_WIDTH, &format!("{what}: right"));
    assert_close(
        rect.bottom,
        bottom / PAGE_HEIGHT,
        &format!("{what}: bottom"),
    );
}

/// `quad` is the glyph box whose corners are given in points of a displayed
/// page `width` by `height` points, measured from its top left corner, in
/// the order of `TextChar::quad`.
pub fn assert_quad(
    quad: [(f64, f64); 4],
    expected: [(f64, f64); 4],
    (width, height): (f64, f64),
    what: &str,
) {
    for (corner, ((x, y), (want_x, want_y))) in quad.into_iter().zip(expected).enumerate() {
        assert_close(x, want_x / width, &format!("{what}: corner {corner} x"));
        assert_close(y, want_y / height, &format!("{what}: corner {corner} y"));
    }
}

pub fn open(fixture: &Fixture) -> Document {
    fixture.open()
}
