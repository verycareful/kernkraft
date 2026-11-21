// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Documents for the tests of page operations and rendering, written
//! byte by byte so that every expectation follows from how a document
//! is built. Page `n` of each draws the text `P<n>` in an uncompressed
//! content stream, which [`markers`] reads back.
//!
//! Object numbers: 1 catalog, 2 root page tree, 3 font, 4 resources; page
//! `i` (counted from 0) is object `10 + 2i` and its content `11 + 2i`.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::borrow::Cow;

use kernkraft_pdf::doc::Document;
use kernkraft_pdf::syntax::{Object, ObjectId, Value};
use kernkraft_pdf::write::{compact, WriteOptions};

use super::pdf_fixture::{stream, table_file, Fixture, OPTIONS};

pub const NESTED_MEDIABOX: [i64; 4] = [0, 0, 200, 300];
pub const NESTED_CROPBOX: [i64; 4] = [0, 0, 150, 150];
pub const NESTED_ROTATE: i64 = 90;
pub const PAGE1_MEDIABOX: [i64; 4] = [0, 0, 100, 100];

/// The seed of every write in the tests.
pub const SEED: [u8; 32] = [7; 32];

pub fn page_object(index: u32) -> u32 {
    10 + 2 * index
}

pub fn id(number: u32) -> ObjectId {
    ObjectId {
        number,
        generation: 0,
    }
}

fn font_and_resources() -> Vec<(u32, Vec<u8>)> {
    vec![
        (
            3,
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        ),
        (4, b"<< /Font << /F1 3 0 R >> >>".to_vec()),
    ]
}

fn content(index: u32) -> (u32, Vec<u8>) {
    let ops = format!("BT /F1 12 Tf 10 10 Td (P{}) Tj ET", index + 1);
    (page_object(index) + 1, stream("", ops.as_bytes()))
}

fn rectangle(r: [i64; 4]) -> String {
    format!("[{} {} {} {}]", r[0], r[1], r[2], r[3])
}

/// A flat document: page `n` has MediaBox `[0 0 w h]` and shared
/// Resources.
pub fn pdf(sizes: &[(f32, f32)]) -> Vec<u8> {
    flat(sizes, "", "", "").bytes
}

/// `n` Letter-sized pages.
pub fn pdf_n(n: u32) -> Vec<u8> {
    pdf(&vec![(612.0, 792.0); n as usize])
}

/// A flat document with `catalog` added to the catalog's entries, `page`
/// to each page's, and `trailer` to the trailer's.
pub fn flat(sizes: &[(f32, f32)], catalog: &str, page: &str, trailer: &str) -> Fixture {
    flat_with(sizes, catalog, page, trailer, &[])
}

/// As [`flat`], with `extra` objects.
pub fn flat_with(
    sizes: &[(f32, f32)],
    catalog: &str,
    page: &str,
    trailer: &str,
    extra: &[(u32, &str)],
) -> Fixture {
    let mut objects = font_and_resources();
    for (number, body) in extra {
        objects.push((*number, body.as_bytes().to_vec()));
    }
    let kids: Vec<String> = (0..sizes.len() as u32)
        .map(|i| format!("{} 0 R", page_object(i)))
        .collect();
    objects.push((
        1,
        format!("<< /Type /Catalog /Pages 2 0 R {catalog} >>").into_bytes(),
    ));
    objects.push((
        2,
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.join(" "),
            sizes.len()
        )
        .into_bytes(),
    ));
    for (i, (w, h)) in sizes.iter().enumerate() {
        let i = i as u32;
        objects.push((
            page_object(i),
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Resources 4 0 R /Contents {} 0 R {page} >>",
                page_object(i) + 1
            )
            .into_bytes(),
        ));
        objects.push(content(i));
    }
    objects.sort_by_key(|(n, _)| *n);
    let borrowed: Vec<(u32, &[u8])> = objects.iter().map(|(n, b)| (*n, b.as_slice())).collect();
    table_file(&borrowed, &format!("/Root 1 0 R {trailer}"))
}

/// A nested page tree:
///
/// ```text
/// root Pages (CropBox NESTED_CROPBOX)
///   page 1 (own MediaBox PAGE1_MEDIABOX, own Resources)
///   intermediate Pages (MediaBox NESTED_MEDIABOX, Resources, Rotate 90)
///     page 2 (nothing of its own)
///     page 3 (nothing of its own)
/// ```
pub fn nested_pdf() -> Vec<u8> {
    let mut objects = font_and_resources();
    objects.push((1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()));
    objects.push((
        2,
        format!(
            "<< /Type /Pages /Count 3 /Kids [10 0 R 5 0 R] /CropBox {} >>",
            rectangle(NESTED_CROPBOX)
        )
        .into_bytes(),
    ));
    objects.push((
        5,
        format!(
            "<< /Type /Pages /Parent 2 0 R /Count 2 /Kids [12 0 R 14 0 R] /MediaBox {} /Resources 4 0 R /Rotate {NESTED_ROTATE} >>",
            rectangle(NESTED_MEDIABOX)
        )
        .into_bytes(),
    ));
    objects.push((
        10,
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox {} /Resources 4 0 R /Contents 11 0 R >>",
            rectangle(PAGE1_MEDIABOX)
        )
        .into_bytes(),
    ));
    objects.push(content(0));
    objects.push((
        12,
        b"<< /Type /Page /Parent 5 0 R /Contents 13 0 R >>".to_vec(),
    ));
    objects.push(content(1));
    objects.push((
        14,
        b"<< /Type /Page /Parent 5 0 R /Contents 15 0 R >>".to_vec(),
    ));
    objects.push(content(2));
    objects.sort_by_key(|(n, _)| *n);
    let borrowed: Vec<(u32, &[u8])> = objects.iter().map(|(n, b)| (*n, b.as_slice())).collect();
    table_file(&borrowed, "/Root 1 0 R").bytes
}

/// Three pages with every document-wide structure a page operation keeps
/// in agreement. Pages are objects 10, 12 and 14; `tag` prefixes the names
/// of the fields, named destinations and attachments, so that two of these
/// merge with collisions or without.
///
/// - links: page 1 has a link (object 21) to page 3 by an explicit
///   destination; page 3 has a link (24) to page 1 by the name `<tag>-p1`;
/// - outline (object 60): `One` (61) to page 1 with the child `One.A` (64)
///   to page 2; `Two` (62) to page 2; `Three` (63) to page 3;
/// - named destinations: the name tree 70 has `<tag>-p1` to 1, `<tag>-p2`
///   to 2 and `<tag>-p3` to 3 (as a dictionary); the catalog's `Dests`
///   (75) has `<tag>old2` to page 2;
/// - form (80): the field `<tag>_f2` is the widget 22 of page 2, the field
///   `<tag>_f3` the widget 23 of page 3;
/// - structure tree (100): elements 101, 102 and 103 for pages 1, 2 and 3
///   (`MCID` 0 on each), `ParentTree` 104 with keys 0, 1 and 2 (the
///   pages' `StructParents`);
/// - page labels: page 1 is `i`, pages 2 and 3 are `A-1` and `A-2`;
/// - an attachment `<tag>-att.txt` in the `EmbeddedFiles` tree (90).
pub fn features(tag: &str) -> Fixture {
    let catalog = format!(
        "/Outlines 60 0 R /Names << /Dests 70 0 R /EmbeddedFiles 90 0 R >> /Dests 75 0 R \
         /AcroForm 80 0 R /StructTreeRoot 100 0 R /MarkInfo << /Marked true >> \
         /PageLabels << /Nums [0 << /S /r >> 1 << /S /D /P ({tag}A-) >>] >> /OpenAction [14 0 R /Fit]"
    );
    let mut objects = font_and_resources();
    objects.push((
        1,
        format!("<< /Type /Catalog /Pages 2 0 R {catalog} >>").into_bytes(),
    ));
    objects.push((
        2,
        b"<< /Type /Pages /Kids [10 0 R 12 0 R 14 0 R] /Count 3 >>".to_vec(),
    ));
    for (index, annots) in [(0u32, "21 0 R"), (1, "22 0 R"), (2, "23 0 R 24 0 R")] {
        objects.push((
            page_object(index),
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources 4 0 R \
                 /Contents {} 0 R /StructParents {index} /Annots [{annots}] >>",
                page_object(index) + 1
            )
            .into_bytes(),
        ));
        objects.push(content(index));
    }
    objects.push((
        21,
        b"<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] /Dest [14 0 R /Fit] >>".to_vec(),
    ));
    objects.push((
        22,
        format!(
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T ({tag}_f2) /Rect [10 10 100 30] /P 12 0 R >>"
        )
        .into_bytes(),
    ));
    objects.push((
        23,
        format!(
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T ({tag}_f3) /Rect [10 10 100 30] /P 14 0 R >>"
        )
        .into_bytes(),
    ));
    objects.push((
        24,
        format!(
            "<< /Type /Annot /Subtype /Link /Rect [0 20 10 30] /A << /S /GoTo /D ({tag}-p1) >> >>"
        )
        .into_bytes(),
    ));
    objects.push((
        60,
        b"<< /Type /Outlines /First 61 0 R /Last 63 0 R /Count 4 >>".to_vec(),
    ));
    objects.push((
        61,
        format!(
            "<< /Title ({tag}One) /Parent 60 0 R /Next 62 0 R /First 64 0 R /Last 64 0 R /Count 1 /Dest [10 0 R /Fit] >>"
        )
        .into_bytes(),
    ));
    objects.push((
        62,
        format!(
            "<< /Title ({tag}Two) /Parent 60 0 R /Prev 61 0 R /Next 63 0 R /Dest [12 0 R /Fit] >>"
        )
        .into_bytes(),
    ));
    objects.push((
        63,
        format!("<< /Title ({tag}Three) /Parent 60 0 R /Prev 62 0 R /Dest ({tag}-p3) >>")
            .into_bytes(),
    ));
    objects.push((
        64,
        format!("<< /Title ({tag}One.A) /Parent 61 0 R /Dest [12 0 R /Fit] >>").into_bytes(),
    ));
    objects.push((
        70,
        format!(
            "<< /Names [({tag}-p1) [10 0 R /Fit] ({tag}-p2) [12 0 R /Fit] ({tag}-p3) << /D [14 0 R /Fit] >>] >>"
        )
        .into_bytes(),
    ));
    objects.push((75, format!("<< /{tag}old2 [12 0 R /Fit] >>").into_bytes()));
    objects.push((
        80,
        b"<< /Fields [22 0 R 23 0 R] /CO [22 0 R 23 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 3 0 R >> >> >>".to_vec(),
    ));
    objects.push((
        90,
        format!("<< /Names [({tag}-att.txt) 91 0 R] >>").into_bytes(),
    ));
    objects.push((
        91,
        format!("<< /Type /Filespec /F ({tag}-att.txt) /EF << /F 92 0 R >> >>").into_bytes(),
    ));
    objects.push((92, stream("/Type /EmbeddedFile", b"attached")));
    objects.push((
        100,
        b"<< /Type /StructTreeRoot /K [101 0 R 102 0 R 103 0 R] /ParentTree 104 0 R /ParentTreeNextKey 3 >>"
            .to_vec(),
    ));
    for (element, page) in [(101, 10), (102, 12), (103, 14)] {
        objects.push((
            element,
            format!("<< /Type /StructElem /S /P /P 100 0 R /Pg {page} 0 R /K 0 >>").into_bytes(),
        ));
    }
    objects.push((
        104,
        b"<< /Nums [0 [101 0 R] 1 [102 0 R] 2 [103 0 R]] >>".to_vec(),
    ));
    objects.sort_by_key(|(n, _)| *n);
    let borrowed: Vec<(u32, &[u8])> = objects.iter().map(|(n, b)| (*n, b.as_slice())).collect();
    table_file(&borrowed, "/Root 1 0 R")
}

/// The bytes of `document` written as a compacted copy.
pub fn saved(document: &Document) -> Vec<u8> {
    compact(
        document,
        &WriteOptions { seed: SEED },
        &kernkraft_fs::NoProgress,
    )
    .expect("the document is written")
}

/// `document` written as a compacted copy and read again, as a user
/// would save and reopen it.
pub fn reopen(document: &Document) -> Document {
    let bytes = compact(
        document,
        &WriteOptions { seed: SEED },
        &kernkraft_fs::NoProgress,
    )
    .expect("the document is written");
    Document::from_bytes(bytes, &OPTIONS, "").expect("the copy opens")
}

/// The `P<n>` marker of each page, in page order.
pub fn markers(document: &Document) -> Vec<String> {
    (1..=document.page_count().unwrap())
        .map(|number| {
            let page = document.page(number).unwrap();
            let contents = page.dictionary.get(b"Contents").expect("page contents");
            let text = content_text(document, contents);
            let start = text.find("(P").expect("a marker") + 1;
            let end = start + text[start..].find(')').unwrap();
            text[start..end].to_string()
        })
        .collect()
}

fn content_text(document: &Document, contents: &Object) -> String {
    let resolved = document.resolve_any(contents);
    match resolved {
        Resolved::Stream(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Resolved::Array(items) => items
            .iter()
            .map(|item| content_text(document, item))
            .collect::<Vec<_>>()
            .join(" "),
        Resolved::Other => String::new(),
    }
}

enum Resolved {
    Stream(Vec<u8>),
    Array(Vec<Object>),
    Other,
}

trait ResolveAny {
    fn resolve_any(&self, object: &Object) -> Resolved;
}

impl ResolveAny for Document {
    fn resolve_any(&self, object: &Object) -> Resolved {
        let Value::Reference(id) = object.value() else {
            return match object.value() {
                Value::Array(items) => Resolved::Array(items.clone()),
                _ => Resolved::Other,
            };
        };
        let loaded = self.object(*id).unwrap().expect("a content object");
        match loaded.stream() {
            Some(stream) => Resolved::Stream(match self.stream_data(*id, stream).unwrap() {
                Cow::Borrowed(bytes) => bytes.to_vec(),
                Cow::Owned(bytes) => bytes,
            }),
            None => match loaded.object().map(Object::value) {
                Some(Value::Array(items)) => Resolved::Array(items.clone()),
                _ => Resolved::Other,
            },
        }
    }
}
