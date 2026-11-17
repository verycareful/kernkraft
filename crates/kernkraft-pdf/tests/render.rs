// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The renderer on the files the page operations and the writer make: every
//! page of a file that was extracted, merged, rotated or compacted is drawn
//! at the size its boxes and rotation give. The drawing itself is tested in
//! `raster/`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "common/mod.rs"]
mod common;
#[path = "pdf_fixture/mod.rs"]
mod pdf_fixture;

use kernkraft_fs::{Expect, NoProgress};
use kernkraft_pdf::doc::Document;
use kernkraft_pdf::render::{render_page, RenderOptions};
use kernkraft_pdf::write::{save_compact, WriteOptions};
use kernkraft_pdf::{extract, merge, rotate, Rotation};

use common::*;
use pdf_fixture::{borrowed, pages_document, stream_file, table_file, OPTIONS};

/// Pixels to a point of the renders of these tests.
const SCALE: f64 = 0.25;

fn open(bytes: &[u8]) -> Document {
    Document::from_bytes(bytes.to_vec(), &OPTIONS, "").unwrap()
}

fn options(scale: f64) -> RenderOptions<'static> {
    RenderOptions {
        scale,
        ..Default::default()
    }
}

/// `document` saved compactly and opened again.
fn saved_and_opened(document: &Document) -> Document {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("saved.pdf");
    save_compact(
        document,
        &path,
        Expect::Anything,
        &WriteOptions { seed: SEED },
        &NoProgress,
    )
    .unwrap();
    open(&std::fs::read(&path).unwrap())
}

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

/// The pixels `points` of a page take at [`SCALE`], whole pixels rounded up.
fn pixels(points: f64) -> u32 {
    (points * SCALE).ceil() as u32
}

#[test]
fn every_corpus_page_renders_at_the_size_its_boxes_give() {
    for f in corpus() {
        let doc = open(&f.bytes);
        assert_eq!(doc.page_count().unwrap(), f.pages, "{}", f.name);
        for n in 1..=f.pages {
            let (w, h) = displayed_size(&doc, n);
            let raster = render_page(&doc, n, &options(SCALE), &NoProgress).unwrap();
            assert_eq!(
                (raster.width, raster.height),
                (pixels(w), pixels(h)),
                "{} page {n}",
                f.name
            );
            assert_eq!(
                raster.rgba.len(),
                (raster.width * raster.height * 4) as usize
            );
        }
    }
}

#[test]
fn rotated_output_renders_swapped() {
    const LETTER: (f64, f64) = (612.0, 792.0);
    let doc = open(&pdf_n(1));
    let turned = saved_and_opened(&rotate(&doc, &[1], Rotation::Cw90).unwrap());
    let raster = render_page(&turned, 1, &options(SCALE), &NoProgress).unwrap();
    assert_eq!(
        (raster.width, raster.height),
        (pixels(LETTER.1), pixels(LETTER.0))
    );
}

#[test]
fn extracted_nested_page_uses_inherited_cropbox() {
    let doc = open(&nested_pdf());
    let part = saved_and_opened(&extract(&doc, &[2]).unwrap());
    // NESTED_CROPBOX is square and lies inside NESTED_MEDIABOX.
    let raster = render_page(&part, 1, &options(1.0), &NoProgress).unwrap();
    assert_eq!((raster.width, raster.height), (150, 150));
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
    let doc = open(&table_file(&objects, "/Root 1 0 R").bytes);
    let raster = render_page(&doc, 1, &options(1.0), &NoProgress).unwrap();
    assert_eq!((raster.width, raster.height), (100, 100));
    let pixel = |x: u32, y: u32| {
        let at = ((y * raster.width + x) * 4) as usize;
        <[u8; 4]>::try_from(&raster.rgba[at..at + 4]).unwrap()
    };
    assert_eq!(pixel(25, 25), [0, 0, 0, 255], "inside the square");
    assert_eq!(pixel(75, 75), [255, 255, 255, 255], "outside it");
}
