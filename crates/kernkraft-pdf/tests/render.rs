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

const CORPUS_WIDTH: u32 = 100;

// The rendered height is the width times the page's aspect ratio, rounded
// to the nearest pixel, as Kern Android's `renderPage` computes it (the
// contract `render_page` matches).

fn number(o: &kernkraft_pdf::lopdf::Object) -> f32 {
    match o {
        kernkraft_pdf::lopdf::Object::Integer(i) => *i as f32,
        kernkraft_pdf::lopdf::Object::Real(r) => *r,
        other => panic!("not a number: {other:?}"),
    }
}

fn rect(o: Option<kernkraft_pdf::lopdf::Object>) -> Option<[f32; 4]> {
    let items = o?.as_array().ok()?.clone();
    let v: Vec<f32> = items.iter().map(number).collect();
    Some([
        v[0].min(v[2]),
        v[1].min(v[3]),
        v[0].max(v[2]),
        v[1].max(v[3]),
    ])
}

/// Displayed size of page `n` in points: the CropBox clipped to the
/// MediaBox (ISO 32000-1, 14.11.2), the CropBox defaulting to the
/// MediaBox, with width and height swapped for a quarter or three-quarter
/// turn (7.7.3.3, `/Rotate`).
fn displayed_size(doc: &kernkraft_pdf::lopdf::Document, n: u32) -> (f32, f32) {
    let media = rect(corpus::effective(doc, n, b"MediaBox")).expect("every page has a MediaBox");
    let crop = rect(corpus::effective(doc, n, b"CropBox")).unwrap_or(media);
    let w = crop[2].min(media[2]) - crop[0].max(media[0]);
    let h = crop[3].min(media[3]) - crop[1].max(media[1]);
    let turns = match corpus::effective(doc, n, b"Rotate") {
        Some(o) => number(&o) as i64,
        None => 0,
    };
    if turns.rem_euclid(180) == 90 {
        (h, w)
    } else {
        (w, h)
    }
}

#[test]
fn every_corpus_page_renders() {
    let dir = tempfile::tempdir().unwrap();
    for f in corpus::corpus() {
        let doc = kernkraft_pdf::load_bytes(&f.bytes).unwrap();
        let path = write(&dir, &format!("{}.pdf", f.name), &f.bytes);
        let renderer = Renderer::open(&path).unwrap();
        assert_eq!(renderer.page_count(), f.pages, "{}", f.name);
        for n in 1..=f.pages {
            let (w, h) = displayed_size(&doc, n);
            let page = renderer.render_page(n, CORPUS_WIDTH).unwrap();
            let expected = (h * CORPUS_WIDTH as f32 / w).round() as u32;
            assert_eq!(
                (page.width, page.height),
                (CORPUS_WIDTH, expected),
                "{} page {n}",
                f.name
            );
        }
    }
}

#[test]
fn rotated_output_renders_swapped() {
    const LETTER: (f32, f32) = (612.0, 792.0);
    let dir = tempfile::tempdir().unwrap();
    let mut doc = kernkraft_pdf::load_bytes(&pdf_n(1)).unwrap();
    kernkraft_pdf::rotate(&mut doc, &[1], kernkraft_pdf::Rotation::Cw90).unwrap();
    let path = dir.path().join("turned.pdf");
    kernkraft_pdf::save(&mut doc, &path, None).unwrap();
    let page = render_page(&path, 1, CORPUS_WIDTH).unwrap();
    let expected = (LETTER.0 * CORPUS_WIDTH as f32 / LETTER.1).round() as u32;
    assert_eq!((page.width, page.height), (CORPUS_WIDTH, expected));
}

#[test]
fn flattened_nested_page_uses_inherited_cropbox() {
    let dir = tempfile::tempdir().unwrap();
    let mut doc = kernkraft_pdf::load_bytes(&nested_pdf()).unwrap();
    kernkraft_pdf::flatten_page_tree(&mut doc).unwrap();
    let path = dir.path().join("flat.pdf");
    kernkraft_pdf::save(&mut doc, &path, None).unwrap();
    // NESTED_CROPBOX is square and lies inside NESTED_MEDIABOX.
    let page = render_page(&path, 2, CORPUS_WIDTH).unwrap();
    assert_eq!((page.width, page.height), (CORPUS_WIDTH, CORPUS_WIDTH));
}

/// A CropBox away from the origin: what lies in its top-left corner is
/// drawn in the rendered page's top-left corner.
#[test]
fn offset_cropbox_maps_to_the_pixmap_origin() {
    let bytes = edited(&pdf_n(1), |doc, pages| {
        // A black square filling the top-left quarter of the crop box
        // [100 100 200 200]; PDF y grows upward, so its top is y = 200.
        let ops = b"0 g 100 150 50 50 re f".to_vec();
        let content = doc.add_object(kernkraft_pdf::lopdf::Stream::new(
            kernkraft_pdf::lopdf::dictionary! {},
            ops,
        ));
        let page = doc.get_dictionary_mut(pages[0]).unwrap();
        page.set("MediaBox", rect_of([0, 0, 300, 300]));
        page.set("CropBox", rect_of([100, 100, 200, 200]));
        page.set("Contents", content);
    });
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "crop.pdf", &bytes);
    let page = render_page(&path, 1, CORPUS_WIDTH).unwrap();
    assert_eq!((page.width, page.height), (CORPUS_WIDTH, CORPUS_WIDTH));
    let quarter = CORPUS_WIDTH / 4;
    assert_eq!(
        pixel(&page, quarter, quarter),
        [0, 0, 0, 255],
        "inside the square"
    );
    assert_eq!(
        pixel(&page, 3 * quarter, 3 * quarter),
        [255, 255, 255, 255],
        "outside it"
    );
}
