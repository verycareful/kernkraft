// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Embedded CMaps (`cmap.rs`, ISO 32000-1:2008, 9.7.5.4, 9.10.3).

use super::*;

/// A Type 0 font (60) whose `Encoding` is the CMap stream (61) with
/// `body` between the codespace range and the end.
fn embedded(stream_entries: &str, body: &str) -> Vec<(u32, Vec<u8>)> {
    let cmap = format!(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
         /CIDSystemInfo 3 dict dup begin /Registry (Adobe) def /Ordering (Japan1) def \
         /Supplement 2 def end def\n/CMapName /Test def /CMapType 1 def\n{body}\n\
         endcmap CMapName currentdict /CMap defineresource pop end end"
    );
    vec![
        (
            60,
            b"<< /Type /Font /Subtype /Type0 /BaseFont /Foo /Encoding 61 0 R \
              /DescendantFonts [62 0 R] >>"
                .to_vec(),
        ),
        (
            61,
            stream(
                &format!("/Type /CMap /CMapName /Test {stream_entries}"),
                cmap.as_bytes(),
            ),
        ),
        (
            62,
            b"<< /Type /Font /Subtype /CIDFontType0 /BaseFont /Foo \
              /CIDSystemInfo << /Registry (Adobe) /Ordering (Japan1) /Supplement 2 >> >>"
                .to_vec(),
        ),
    ]
}

fn read(objects: &[(u32, Vec<u8>)]) -> (Document, Font) {
    let borrowed: Vec<(u32, &[u8])> = objects.iter().map(|(n, b)| (*n, b.as_slice())).collect();
    font_of(&borrowed, 60)
}

/// One- and two-byte codes, a range, a single code and a notdef range.
#[test]
fn an_embedded_cmap_maps_codes_to_cids() {
    let (document, font) = read(&embedded(
        "",
        "2 begincodespacerange <00> <7F> <8000> <FFFF> endcodespacerange\n\
         1 beginnotdefrange <00> <1F> 1 endnotdefrange\n\
         2 begincidrange <20> <7E> 100 <8100> <81FF> 1000 endcidrange\n\
         1 begincidchar <8142> 7 endcidchar",
    ));
    let chars: Vec<_> = font
        .chars(&[0x41, 0x81, 0x10, 0x81, 0x42, 0x0A, 0x7F])
        .collect();
    let got: Vec<_> = chars.iter().map(|c| (c.code, c.len, c.cid)).collect();
    // 0x41 is 100 + 0x21; 0x8110 is 1000 + 0x10; 0x8142 is the char entry,
    // listed after the range that also covers it, so the range's CID
    // stays; 0x0A is in the notdef range (CID 1); 0x7F has no mapping
    // (CID 0).
    assert_eq!(
        got,
        [
            (0x41, 1, 133),
            (0x8110, 2, 1016),
            (0x8142, 2, 1000 + 0x42),
            (0x0A, 1, 1),
            (0x7F, 1, 0)
        ]
    );
    assert!(!font.is_vertical());
    assert_eq!(font.encoding_cmap().and_then(|c| c.name()), Some("Test"));
    let info = font
        .encoding_cmap()
        .and_then(|c| c.system_info())
        .expect("system info");
    assert_eq!(
        (
            info.registry.as_str(),
            info.ordering.as_str(),
            info.supplement
        ),
        ("Adobe", "Japan1", 2)
    );
    assert_eq!(
        of_kind(&document, &DiagnosticKind::CMapRangesOverlap).len(),
        1
    );
}

/// 9.7.5.2: `UseCMap` of the stream's dictionary adds the CMap named; an
/// entry of the CMap itself wins over it. 90ms-RKSJ-H maps 0x8140 to 633,
/// 0x8141 to 634.
#[test]
fn use_cmap_adds_to_the_named_cmap() {
    let (_, font) = read(&embedded(
        "/UseCMap /90ms-RKSJ-H",
        "1 begincidchar <8140> 5 endcidchar",
    ));
    let chars: Vec<_> = font.chars(&[0x81, 0x40, 0x81, 0x41, 0x41]).collect();
    let got: Vec<_> = chars.iter().map(|c| (c.code, c.len, c.cid)).collect();
    // The codespace is the parent's: 0x41 is a one-byte code of its range.
    assert_eq!(got, [(0x8140, 2, 5), (0x8141, 2, 634), (0x41, 1, 264)]);
}

/// The same through the `usecmap` operator of the CMap's own data.
#[test]
fn the_usecmap_operator_adds_to_the_named_cmap() {
    let (_, font) = read(&embedded(
        "",
        "/90ms-RKSJ-H usecmap\n1 begincidchar <8141> 9 endcidchar",
    ));
    let got: Vec<_> = font
        .chars(&[0x81, 0x40, 0x81, 0x41])
        .map(|c| c.cid)
        .collect();
    assert_eq!(got, [633, 9]);
}

#[test]
fn an_unknown_usecmap_is_refused() {
    let objects = embedded("", "/No-Such usecmap\n1 begincidchar <41> 9 endcidchar");
    let borrowed: Vec<(u32, &[u8])> = objects.iter().map(|(n, b)| (*n, b.as_slice())).collect();
    let document = font_document(&borrowed).open();
    assert!(matches!(
        load(&document, 60),
        Err(PdfError::UnknownCMap { .. })
    ));
}

/// Table 120: `WMode` of the stream dictionary sets vertical writing, and
/// so does the CMap's own `WMode`.
#[test]
fn wmode_sets_vertical_writing() {
    let (_, from_dictionary) = read(&embedded(
        "/WMode 1",
        "1 begincodespacerange <0000> <FFFF> endcodespacerange",
    ));
    assert!(from_dictionary.is_vertical());
    let (_, from_data) = read(&embedded(
        "",
        "/WMode 1 def 1 begincodespacerange <0000> <FFFF> endcodespacerange",
    ));
    assert!(from_data.is_vertical());
}

#[test]
fn entries_that_are_not_entries_are_reported_and_skipped() {
    let (document, font) = read(&embedded(
        "",
        "1 begincodespacerange <00> <FF> endcodespacerange\n\
         5 begincidrange\n\
         <20> <2F> 10\n\
         <3000> <30FF> 5000\n\
         <40> <3F> 60\n\
         <50> /name 70\n\
         <60> <6F> 100\n\
         endcidrange",
    ));
    // Entries 2 (codes of different lengths), 3 (the range runs backward)
    // and 4 (an operand of the wrong kind) are skipped.
    let cids: Vec<u32> = font
        .chars(&[0x20, 0x41, 0x50, 0x60])
        .map(|c| c.cid)
        .collect();
    assert_eq!(cids, [10, 0, 0, 100]);
    assert!(!of_kind(&document, &DiagnosticKind::CMapEntryInvalid).is_empty());
}

#[test]
fn a_block_that_is_not_ended_is_reported() {
    let (document, font) = read(&embedded(
        "",
        "1 begincodespacerange <00> <FF> endcodespacerange\n1 begincidrange <20> <2F> 10",
    ));
    assert_eq!(font.chars(&[0x20]).next().unwrap().cid, 10);
    assert_eq!(
        of_kind(&document, &DiagnosticKind::CMapUnterminated).len(),
        1
    );
}

/// 9.10.3: a ToUnicode CMap that cannot be read leaves the font without
/// one, and says so.
#[test]
fn an_unreadable_to_unicode_cmap_is_reported() {
    let (document, font) = font_of(
        &[
            (
                60,
                b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /ToUnicode 61 0 R >>",
            ),
            (61, &stream("", b"1 beginbfchar <41> (unterminated string")),
        ],
        60,
    );
    assert_eq!(
        of_kind(&document, &DiagnosticKind::ToUnicodeUnreadable).len(),
        1
    );
    // The glyph name still gives the text.
    assert_eq!(texts(&font, b"A"), [Some("A".into())]);
}

/// A bf range's last code unit counts up from one code to the next, and a
/// destination of several units gives several characters.
#[test]
fn bf_ranges_count_up_in_the_last_unit() {
    let (_, font) = font_of(
        &[
            (
                60,
                b"<< /Type /Font /Subtype /Type1 /BaseFont /Foo /ToUnicode 61 0 R \
                   /FirstChar 0 /Widths [] >>",
            ),
            (
                61,
                &to_unicode(
                    "1 beginbfrange <0041> <0043> <00660069> endbfrange\n\
                     1 beginbfchar <0050> <D840DC0B> endbfchar",
                ),
            ),
        ],
        60,
    );
    assert_eq!(
        texts(&font, b"ABCP"),
        [
            Some("fi".into()),
            Some("fj".into()),
            Some("fk".into()),
            Some("\u{2000B}".into())
        ]
    );
}
