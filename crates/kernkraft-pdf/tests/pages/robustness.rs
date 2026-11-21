// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Deterministic mutation of the document every structure is in: each
//! truncation and a substitution of bytes that mean something to the
//! reader. A mutated file must open or be refused, and every page
//! operation on a document that opened must give a document or an error:
//! a panic or a hang fails the test.

use kernkraft_fs::NoProgress;
use kernkraft_pdf::{
    extract, merge, remove, reorder, rotate, split_by_bookmarks, split_per_page, Rotation,
};

use super::*;

/// Bytes that change how the reader takes what follows.
const REPLACEMENTS: &[u8] = b"()<>[]/ 9R\xFF";
/// One offset in this many is substituted, so that the run stays short.
const STRIDE: usize = 7;

fn exercise(bytes: &[u8]) {
    let Ok(document) = Document::from_bytes(bytes.to_vec(), &OPTIONS, "") else {
        return;
    };
    let Ok(count) = document.page_count() else {
        return;
    };
    let _ = rotate(&document, &[], Rotation::Cw90);
    if count >= 2 {
        let _ = remove(&document, &[1]);
        let order: Vec<u32> = (1..=count).rev().collect();
        let _ = reorder(&document, &order);
        let _ = merge(&[&document, &document], &OPTIONS, &NoProgress);
    }
    if count >= 1 {
        let _ = extract(&document, &[count]);
    }
    let _ = split_per_page(&document, &NoProgress);
    let _ = split_by_bookmarks(&document);
}

fn fixture() -> Vec<u8> {
    features("").bytes
}

#[test]
fn every_truncation() {
    let fixture = fixture();
    for length in 0..fixture.len() {
        exercise(&fixture[..length]);
    }
}

#[test]
fn substituted_bytes() {
    let fixture = fixture();
    let mut bytes = fixture.clone();
    for offset in (0..fixture.len()).step_by(STRIDE) {
        for &replacement in REPLACEMENTS {
            bytes[offset] = replacement;
            exercise(&bytes);
        }
        bytes[offset] = fixture[offset];
    }
}

/// A chain of structure elements nested far deeper than the tree is read:
/// the walk ends with an error, not a stack overflow.
#[test]
fn a_structure_tree_nested_too_deeply_is_refused() {
    const DEPTH: u32 = 3000;
    let mut extra: Vec<(u32, String)> =
        vec![(30, "<< /Type /StructTreeRoot /K 31 0 R >>".to_string())];
    for n in 0..DEPTH {
        let number = 31 + n;
        let child = if n + 1 < DEPTH {
            format!("/K {} 0 R", number + 1)
        } else {
            "/Pg 12 0 R /K 0".to_string()
        };
        extra.push((
            number,
            format!(
                "<< /Type /StructElem /S /Div /P {} 0 R {child} >>",
                number - 1
            ),
        ));
    }
    let borrowed: Vec<(u32, &str)> = extra.iter().map(|(n, s)| (*n, s.as_str())).collect();
    let fixture = flat_with(
        &[(10.0, 10.0), (10.0, 10.0)],
        "/StructTreeRoot 30 0 R",
        "",
        "",
        &borrowed,
    );
    let document = open(&fixture.bytes);
    assert!(matches!(
        remove(&document, &[2]),
        Err(kernkraft_pdf::PdfError::StructureTree { .. })
    ));
}

/// A name tree, a form and an outline that lead back to themselves end the
/// operation with an answer.
#[test]
fn trees_that_loop_do_not_hang() {
    let extra = [
        (30, "<< /Kids [30 0 R] >>"),
        (31, "<< /FT /Tx /T (loop) /Kids [31 0 R] >>"),
        (
            32,
            "<< /Type /Outlines /First 33 0 R /Last 33 0 R /Count 1 >>",
        ),
        (
            33,
            "<< /Title (self) /Parent 32 0 R /Next 33 0 R /First 33 0 R /Dest [12 0 R /Fit] >>",
        ),
    ];
    let fixture = flat_with(
        &[(10.0, 10.0), (10.0, 10.0)],
        "/Names << /Dests 30 0 R >> /AcroForm << /Fields [31 0 R] >> /Outlines 32 0 R",
        "",
        "",
        &extra,
    );
    let document = open(&fixture.bytes);
    let _ = remove(&document, &[2]);
    let _ = merge(&[&document, &document], &OPTIONS, &NoProgress);
}
