// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![cfg(feature = "render")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::PathBuf;

use common::*;
use kernkraft_pdf::{page_size_pt, render_page, PdfError, Renderer, MAX_RENDER_DIMENSION};

fn write(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn pixel(page: &kernkraft_pdf::RenderedPage, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * page.width + x) * 4) as usize;
    page.rgba[i..i + 4].try_into().unwrap()
}

#[test]
fn renders_at_requested_width() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "a.pdf", &pdf(&[(200.0, 300.0)]));
    let page = render_page(&path, 1, 400).unwrap();
    assert_eq!((page.width, page.height), (400, 600));
    assert_eq!(page.rgba.len(), 400 * 600 * 4);
}

#[test]
fn render_is_opaque_white_background() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "a.pdf", &pdf(&[(200.0, 300.0)]));
    let page = render_page(&path, 1, 200).unwrap();
    assert!(page.rgba.as_chunks::<4>().0.iter().all(|px| px[3] == 255));
    assert_eq!(pixel(&page, 0, 0), [255, 255, 255, 255]);
}

#[test]
fn render_draws_text() {
    // The fixture draws "P1" at (10, 10) in 12 pt Helvetica, from the bottom
    // left; at 1 px per point that is the bottom-left corner of the image.
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "a.pdf", &pdf(&[(200.0, 300.0)]));
    let page = render_page(&path, 1, 200).unwrap();
    let inked = (8..30)
        .flat_map(|x| (page.height - 24..page.height - 8).map(move |y| (x, y)))
        .any(|(x, y)| pixel(&page, x, y)[0] < 128);
    assert!(inked);
}

#[test]
fn rotated_page_swaps_dimensions() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "n.pdf", &nested_pdf());
    // Page 2 inherits MediaBox 200x300, CropBox 150x150 and Rotate 90; the
    // visible area is the intersection, 150x150.
    let (w, h) = page_size_pt(&path, 2).unwrap();
    assert_eq!((w, h), (150.0, 150.0));
    let path = write(
        &dir,
        "r.pdf",
        &edited(&pdf(&[(200.0, 300.0)]), |doc, pages| {
            doc.get_dictionary_mut(pages[0]).unwrap().set("Rotate", 90);
        }),
    );
    assert_eq!(page_size_pt(&path, 1).unwrap(), (300.0, 200.0));
    let page = render_page(&path, 1, 300).unwrap();
    assert_eq!((page.width, page.height), (300, 200));
}

#[test]
fn renderer_renders_many_pages() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "a.pdf", &pdf(&[(100.0, 100.0), (100.0, 50.0)]));
    let r = Renderer::open(&path).unwrap();
    assert_eq!(r.page_count(), 2);
    assert_eq!(r.render_page(2, 100).unwrap().height, 50);
    assert_eq!(r.render_page(1, 100).unwrap().height, 100);
}

#[test]
fn render_rejects_page_out_of_range() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "a.pdf", &pdf_n(2));
    assert!(matches!(
        render_page(&path, 3, 100),
        Err(PdfError::PageOutOfRange { page: 3, count: 2 })
    ));
    assert!(matches!(
        render_page(&path, 0, 100),
        Err(PdfError::PageOutOfRange { page: 0, .. })
    ));
}

#[test]
fn render_rejects_zero_and_huge_width() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "a.pdf", &pdf_n(1));
    assert!(matches!(
        render_page(&path, 1, 0),
        Err(PdfError::Invalid(_))
    ));
    assert!(matches!(
        render_page(&path, 1, MAX_RENDER_DIMENSION + 1),
        Err(PdfError::Invalid(_))
    ));
}

#[test]
fn render_rejects_too_tall_result() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "tall.pdf", &pdf(&[(10.0, 10_000.0)]));
    assert!(matches!(
        render_page(&path, 1, 100),
        Err(PdfError::Invalid(_))
    ));
}

#[test]
fn render_reports_missing_and_garbage_files() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        render_page(&dir.path().join("none.pdf"), 1, 100),
        Err(PdfError::Fs(kernkraft_fs::FsError::NotFound { .. }))
    ));
    let path = write(&dir, "junk.pdf", b"not a pdf at all");
    assert!(render_page(&path, 1, 100).is_err());
}

#[test]
fn render_refuses_encrypted() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "locked.pdf", &encrypted_stub());
    let err = render_page(&path, 1, 100).unwrap_err();
    assert!(matches!(err, PdfError::Encrypted), "{err:?}");
}
