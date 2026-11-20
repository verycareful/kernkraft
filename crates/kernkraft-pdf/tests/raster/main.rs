// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Tests of the raster renderer, one module per area of `src/render/`.
//! Pages are written byte by byte on a 20 by 20 point page, drawn at one
//! pixel to the point, so a shape on whole numbers covers whole pixels and
//! each expectation is the pixel arithmetic of its content, never a run.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

#[path = "../pdf_fixture/mod.rs"]
mod pdf_fixture;

mod annotations;
mod embed;
mod geometry;
mod images;
mod paths;
mod patterns;
mod robustness;
mod shadings;
mod text;
mod transparency;

use kernkraft_fs::NoProgress;
use kernkraft_pdf::doc::Document;
use kernkraft_pdf::render::{render_page, Raster, RenderOptions};
use kernkraft_pdf::{Diagnostic, DiagnosticKind, PdfError};

pub use pdf_fixture::*;

pub const WHITE: [u8; 4] = [255, 255, 255, 255];
pub const BLACK: [u8; 4] = [0, 0, 0, 255];
pub const RED: [u8; 4] = [255, 0, 0, 255];
pub const BLUE: [u8; 4] = [0, 0, 255, 255];

/// The side of the test page, in points.
pub const SIDE: u32 = 20;

/// A one-page file: `page_entries` go in the page dictionary (a
/// `MediaBox`, a `Rotate`), `resources` in its `Resources`, and `extra`
/// are further objects.
pub fn document(
    page_entries: &str,
    content: &str,
    resources: &str,
    extra: &[(u32, &[u8])],
) -> Document {
    fixture(page_entries, content, resources, extra).open()
}

/// The file of [`document`].
pub fn fixture(
    page_entries: &str,
    content: &str,
    resources: &str,
    extra: &[(u32, &[u8])],
) -> Fixture {
    let mut objects: Vec<(u32, Vec<u8>)> = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [10 0 R] /Count 1 >>".to_vec()),
        (
            10,
            format!(
                "<< /Type /Page /Parent 2 0 R /Contents 11 0 R {page_entries} \
                 /Resources << {resources} >> >>"
            )
            .into_bytes(),
        ),
        (11, stream("", content.as_bytes())),
    ];
    for (number, body) in extra {
        objects.push((*number, body.to_vec()));
    }
    table_file(&borrowed(&objects), "/Root 1 0 R")
}

/// The 20 by 20 point test page.
pub fn square_page(content: &str, resources: &str, extra: &[(u32, &[u8])]) -> Document {
    document("/MediaBox [0 0 20 20]", content, resources, extra)
}

/// A one-page file like [`square_page`] whose catalog turns the optional
/// content group `/L` (object 20, in the page's `Properties`) off. `extra`
/// objects must not use 1, 2, 10, 11 or 20.
pub fn hidden_group_page(
    page_entries: &str,
    content: &str,
    resources: &str,
    extra: &[(u32, &[u8])],
) -> Document {
    let mut objects: Vec<(u32, Vec<u8>)> = vec![
        (
            1,
            b"<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [20 0 R] \
              /D << /OFF [20 0 R] >> >> >>"
                .to_vec(),
        ),
        (2, b"<< /Type /Pages /Kids [10 0 R] /Count 1 >>".to_vec()),
        (
            10,
            format!(
                "<< /Type /Page /Parent 2 0 R /Contents 11 0 R /MediaBox [0 0 20 20] \
                 {page_entries} /Resources << /Properties << /L 20 0 R >> {resources} >> >>"
            )
            .into_bytes(),
        ),
        (11, stream("", content.as_bytes())),
        (20, b"<< /Type /OCG /Name (Layer) >>".to_vec()),
    ];
    for (number, body) in extra {
        objects.push((*number, body.to_vec()));
    }
    objects.sort_by_key(|(n, _)| *n);
    table_file(&borrowed(&objects), "/Root 1 0 R").open()
}

pub fn render(doc: &Document) -> Raster {
    render_with(doc, &RenderOptions::default()).expect("the page renders")
}

/// Opens `bytes` and draws page 1: for the tests that damage a file.
pub fn robustness_render(bytes: Vec<u8>) -> Result<(), PdfError> {
    let doc = Document::from_bytes(bytes, &OPTIONS, "")?;
    render_with(&doc, &RenderOptions::default()).map(drop)
}

pub fn render_with(doc: &Document, options: &RenderOptions<'_>) -> Result<Raster, PdfError> {
    render_page(doc, 1, options, &NoProgress)
}

/// Draws `content` on the test page.
pub fn draw(content: &str) -> Raster {
    render(&square_page(content, "", &[]))
}

/// The pixel at column `x`, row `y` (row 0 is the top).
pub fn px(raster: &Raster, x: u32, y: u32) -> [u8; 4] {
    assert!(
        x < raster.width && y < raster.height,
        "({x}, {y}) is off the picture"
    );
    let at = ((y * raster.width + x) * 4) as usize;
    raster.rgba[at..at + 4].try_into().unwrap()
}

/// Asserts every pixel of the box `[x0, x1) x [y0, y1)` is `colour`.
pub fn assert_box(raster: &Raster, x0: u32, y0: u32, x1: u32, y1: u32, colour: [u8; 4]) {
    for y in y0..y1 {
        for x in x0..x1 {
            assert_eq!(px(raster, x, y), colour, "pixel ({x}, {y})");
        }
    }
}

/// Asserts `found` is within `tolerance` of `wanted` in every channel.
pub fn assert_near(found: [u8; 4], wanted: [u8; 4], tolerance: u8) {
    for (f, w) in found.iter().zip(wanted) {
        assert!(
            f.abs_diff(w) <= tolerance,
            "{found:?} is not near {wanted:?}"
        );
    }
}

pub fn diagnostics_of(doc: &Document, test: impl Fn(&DiagnosticKind) -> bool) -> Vec<Diagnostic> {
    doc.diagnostics()
        .into_iter()
        .filter(|d| test(&d.kind))
        .collect()
}
