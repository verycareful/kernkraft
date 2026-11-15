// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![cfg(feature = "render")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "common/mod.rs"]
mod common;
#[path = "pdf_fixture/mod.rs"]
mod pdf_fixture;

use std::path::PathBuf;

use kernkraft_fs::{Expect, NoProgress};
use kernkraft_pdf::doc::Document;
use kernkraft_pdf::write::{save_compact, WriteOptions};
use kernkraft_pdf::{
    extract, merge, page_size_pt, render_page, rotate, PdfError, Renderer, Rotation,
    MAX_RENDER_DIMENSION,
};

use common::*;
use pdf_fixture::{borrowed, pages_document, stream_file, table_file, OPTIONS};

fn write(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn open(bytes: &[u8]) -> Document {
    Document::from_bytes(bytes.to_vec(), &OPTIONS, "").unwrap()
}

/// `document` saved to `name` in `dir`.
fn save(dir: &tempfile::TempDir, name: &str, document: &Document) -> PathBuf {
    let path = dir.path().join(name);
    save_compact(
        document,
        &path,
        Expect::Anything,
        &WriteOptions { seed: SEED },
        &NoProgress,
    )
    .unwrap();
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
        &flat(&[(200.0, 300.0)], "", "/Rotate 90", "").bytes,
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
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testdata/encrypted/rc4-128.pdf"),
    )
    .unwrap();
    let path = write(&dir, "locked.pdf", &bytes);
    let err = render_page(&path, 1, 100).unwrap_err();
    assert!(matches!(err, PdfError::Encrypted), "{err:?}");
}

const CORPUS_WIDTH: u32 = 100;

// The rendered height is the width times the page's aspect ratio, rounded
// to the nearest pixel, as Kern Android's `renderPage` computes it (the
// contract `render_page` matches).

/// A file kernkraft must read and write, and its page count.
struct Fixture {
    name: &'static str,
    bytes: Vec<u8>,
    pages: u32,
}

fn corpus() -> Vec<Fixture> {
    let plain = pages_document(4);
    // Streams cannot lie in an object stream; the page dictionaries can.
    let in_stream =
        |number: u32, body: &[u8]| number >= 10 && !body.windows(6).any(|w| w == b"stream");
    let compressed: Vec<(u32, Vec<u8>)> = plain
        .iter()
        .filter(|(number, body)| in_stream(*number, body))
        .cloned()
        .collect();
    let kept: Vec<(u32, Vec<u8>)> = plain
        .iter()
        .filter(|(number, body)| !in_stream(*number, body))
        .cloned()
        .collect();
    let object_streams = stream_file(&borrowed(&kept), &borrowed(&compressed), "/Root 1 0 R").bytes;
    let features = open(&features("").bytes);
    let a = open(&features_bytes("A"));
    let b = open(&features_bytes("B"));
    vec![
        Fixture {
            name: "classic",
            bytes: pdf_n(3),
            pages: 3,
        },
        Fixture {
            name: "nested",
            bytes: nested_pdf(),
            pages: 3,
        },
        Fixture {
            name: "object-streams",
            bytes: object_streams,
            pages: 4,
        },
        Fixture {
            name: "compacted",
            bytes: saved(&features),
            pages: 3,
        },
        Fixture {
            name: "extracted",
            bytes: saved(&extract(&features, &[3, 1]).unwrap()),
            pages: 2,
        },
        Fixture {
            name: "merged",
            bytes: saved(&merge(&[&a, &b], &OPTIONS, &NoProgress).unwrap()),
            pages: 6,
        },
    ]
}

fn features_bytes(tag: &str) -> Vec<u8> {
    features(tag).bytes
}

/// Displayed size of page `n` in points: the CropBox clipped to the
/// MediaBox (ISO 32000-1, 14.11.2), the CropBox defaulting to the
/// MediaBox, with width and height swapped for a quarter or three-quarter
/// turn (7.7.3.3, `/Rotate`).
fn displayed_size(doc: &Document, n: u32) -> (f64, f64) {
    let page = doc.page(n).unwrap();
    let media = page.media_box.expect("every page has a MediaBox");
    let crop = page.crop_box.unwrap_or(media);
    let w = crop.urx.min(media.urx) - crop.llx.max(media.llx);
    let h = crop.ury.min(media.ury) - crop.lly.max(media.lly);
    if page.rotate % 180 == 90 {
        (h, w)
    } else {
        (w, h)
    }
}

#[test]
fn every_corpus_page_renders() {
    let dir = tempfile::tempdir().unwrap();
    for f in corpus() {
        let doc = open(&f.bytes);
        let path = write(&dir, &format!("{}.pdf", f.name), &f.bytes);
        let renderer = Renderer::open(&path).unwrap();
        assert_eq!(renderer.page_count(), f.pages, "{}", f.name);
        for n in 1..=f.pages {
            let (w, h) = displayed_size(&doc, n);
            let page = renderer.render_page(n, CORPUS_WIDTH).unwrap();
            let expected = (h * f64::from(CORPUS_WIDTH) / w).round() as u32;
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
    const LETTER: (f64, f64) = (612.0, 792.0);
    let dir = tempfile::tempdir().unwrap();
    let doc = open(&pdf_n(1));
    let turned = rotate(&doc, &[1], Rotation::Cw90).unwrap();
    let path = save(&dir, "turned.pdf", &turned);
    let page = render_page(&path, 1, CORPUS_WIDTH).unwrap();
    let expected = (LETTER.0 * f64::from(CORPUS_WIDTH) / LETTER.1).round() as u32;
    assert_eq!((page.width, page.height), (CORPUS_WIDTH, expected));
}

#[test]
fn extracted_nested_page_uses_inherited_cropbox() {
    let dir = tempfile::tempdir().unwrap();
    let doc = open(&nested_pdf());
    let part = extract(&doc, &[2]).unwrap();
    let path = save(&dir, "part.pdf", &part);
    // NESTED_CROPBOX is square and lies inside NESTED_MEDIABOX.
    let page = render_page(&path, 1, CORPUS_WIDTH).unwrap();
    assert_eq!((page.width, page.height), (CORPUS_WIDTH, CORPUS_WIDTH));
}

/// A CropBox away from the origin: what lies in its top-left corner is
/// drawn in the rendered page's top-left corner.
#[test]
fn offset_cropbox_maps_to_the_pixmap_origin() {
    // A black square filling the top-left quarter of the crop box
    // [100 100 200 200]; PDF y grows upward, so its top is y = 200.
    let ops = pdf_fixture::stream("", b"0 g 100 150 50 50 re f");
    let objects: Vec<(u32, &[u8])> = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>"),
        (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
        (
            3,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /CropBox [100 100 200 200] /Contents 4 0 R >>",
        ),
        (4, ops.as_slice()),
    ];
    let bytes = table_file(&objects, "/Root 1 0 R").bytes;
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
