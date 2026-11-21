// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Deterministic mutation of a tagged page with links and named
//! destinations: every truncation and every substitution of one byte by a
//! byte that means something to the reader. Each mutated file must open or
//! be refused, and each of its text, search, link and destination reads
//! must give a result or an error: a panic or a hang fails the test.

use super::*;
use kernkraft_pdf::text::{SearchOptions, TextExtractor};

/// Bytes that change how the reader takes what follows: delimiters, white
/// space, digits at each end of the range, the letters of `R`, `n` and
/// `f`, and the extreme values.
const REPLACEMENTS: &[u8] = b"()<>[]/% \n\x0009Rnf\xFF";

fn sample() -> Vec<u8> {
    let content =
        "/P << /MCID 0 >> BDC BT /F1 10 Tf 100 700 Td (Hello) Tj 34 0 Td (World) Tj ET EMC \
                   /P << /MCID 1 >> BDC BT /F2 10 Tf 100 600 Td (\\001nd) Tj ET EMC";
    document(
        &[
            PageSpec {
                content,
                entries: "/Annots [<< /Type /Annot /Subtype /Link /Rect [100 700 200 720] \
                          /QuadPoints [100 712 180 712 100 700 180 700] /Dest (a) >>]",
            },
            PageSpec::of("BT /F1 10 Tf 100 700 Td (More) Tj ET"),
        ],
        "/StructTreeRoot 70 0 R /Dests 71 0 R /Names << /Dests 72 0 R >>",
        "",
        &[
            (
                70,
                b"<< /Type /StructTreeRoot /K [73 0 R 74 0 R] /RoleMap << /X /Span >> >>",
            ),
            (73, b"<< /Type /StructElem /S /P /Pg 10 0 R /K 1 >>"),
            (
                74,
                b"<< /Type /StructElem /S /P /K << /Type /MCR /Pg 10 0 R /MCID 0 >> >>",
            ),
            (71, b"<< /b [12 0 R /Fit] >>"),
            (72, b"<< /Names [(a) [10 0 R /XYZ 1 2 3]] >>"),
        ],
    )
    .bytes
}

fn exercise(bytes: &[u8]) {
    let Ok(document) = Document::from_bytes(bytes.to_vec(), &OPTIONS, "") else {
        return;
    };
    let Ok(mut extractor) = TextExtractor::new(&document) else {
        return;
    };
    if let Ok(count) = document.page_count() {
        for number in 1..=count.min(4) {
            if let Ok(page) = extractor.page(number, &NoProgress) {
                let _ = page.search("o", &SearchOptions::default());
                let _ = page.selection(0, page.len().saturating_sub(1));
                let _ = page.text();
                let _ = page.nearest_char(0.2, 0.2);
            }
            if let Ok(links) = document.links(number) {
                for link in &links {
                    if let Some(destination) = link.target() {
                        let _ = document.locate(destination);
                    }
                }
            }
        }
    }
    let _ = document.search("o", &SearchOptions::default(), &NoProgress);
    if let Ok(named) = document.named_destinations() {
        for entry in &named {
            let _ = document.locate(&entry.destination);
        }
    }
}

#[test]
fn the_sample_reads_whole() {
    let document = Document::from_bytes(sample(), &OPTIONS, "").unwrap();
    let page = document.text_page(1, &NoProgress).unwrap();
    assert!(page.tagged);
    assert_eq!(page.text(), "\u{FB01}nd\n\nHello World");
    assert_eq!(document.links(1).unwrap().len(), 1);
    assert_eq!(document.named_destinations().unwrap().len(), 2);
}

#[test]
fn every_truncation_reads_or_is_refused() {
    let sample = sample();
    for length in 0..sample.len() {
        exercise(&sample[..length]);
    }
}

#[test]
fn every_single_byte_substitution_reads_or_is_refused() {
    let sample = sample();
    let mut bytes = sample.clone();
    for offset in 0..sample.len() {
        for &replacement in REPLACEMENTS {
            bytes[offset] = replacement;
            exercise(&bytes);
        }
        bytes[offset] = sample[offset];
    }
}
