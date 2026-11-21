// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Composite fonts (`composite.rs`, ISO 32000-1:2008, 9.7).

use super::*;
use kernkraft_pdf::font::FontKind;

/// A Type 0 font (object 60) with CMap `encoding`, whose one descendant
/// (61) has `cid_entries` and the character collection `ordering`, and a
/// descriptor (62).
fn type0(encoding: &str, subtype: &str, ordering: &str, cid_entries: &str) -> Vec<(u32, Vec<u8>)> {
    vec![
        (
            60,
            format!(
                "<< /Type /Font /Subtype /Type0 /BaseFont /Foo /Encoding {encoding} \
                 /DescendantFonts [61 0 R] >>"
            )
            .into_bytes(),
        ),
        (
            61,
            format!(
                "<< /Type /Font /Subtype /{subtype} /BaseFont /Foo \
                 /CIDSystemInfo << /Registry (Adobe) /Ordering ({ordering}) /Supplement 2 >> \
                 /FontDescriptor 62 0 R {cid_entries} >>"
            )
            .into_bytes(),
        ),
        (
            62,
            b"<< /Type /FontDescriptor /FontName /Foo /Flags 4 >>".to_vec(),
        ),
    ]
}

fn fonts(objects: &[(u32, Vec<u8>)]) -> (Document, Font) {
    let borrowed: Vec<(u32, &[u8])> = objects.iter().map(|(n, b)| (*n, b.as_slice())).collect();
    font_of(&borrowed, 60)
}

/// 9.7.4.3: `W` gives widths for one CID or a range, `DW` the rest.
#[test]
fn identity_h_with_widths_by_cid() {
    let (document, font) = fonts(&type0(
        "/Identity-H",
        "CIDFontType2",
        "Identity",
        "/DW 800 /W [1 [500 600] 10 12 700]",
    ));
    let chars: Vec<_> = font.chars(&[0, 1, 0, 2, 0, 11, 0, 99, 0, 0]).collect();
    let cids: Vec<u32> = chars.iter().map(|c| c.cid).collect();
    assert_eq!(cids, [1, 2, 11, 99, 0]);
    assert_eq!(
        widths(&font, &[0, 1, 0, 2, 0, 11, 0, 99]),
        [500.0, 600.0, 700.0, 800.0]
    );
    assert!(chars.iter().all(|c| c.len == 2 && c.vertical.is_none()));
    assert_eq!(chars[0].code, 1);
    assert!(chars.iter().all(|c| !c.is_space && c.unicode.is_none()));
    assert_eq!(font.kind(), FontKind::Type0);
    assert!(!font.is_vertical());
    assert_eq!(
        font.encoding_cmap().and_then(|c| c.name()),
        Some("Identity-H")
    );
    assert!(document.diagnostics().is_empty());
}

#[test]
fn the_default_width_is_1000() {
    let (_, font) = fonts(&type0("/Identity-H", "CIDFontType2", "Identity", ""));
    assert_eq!(widths(&font, &[0, 5]), [1000.0]);
}

/// 9.7.4.3: the first range given wins where two overlap.
#[test]
fn overlapping_width_ranges_are_reported_and_the_first_wins() {
    let (document, font) = fonts(&type0(
        "/Identity-H",
        "CIDFontType2",
        "Identity",
        "/W [10 20 300 15 25 400]",
    ));
    assert_eq!(widths(&font, &[0, 12, 0, 18, 0, 22]), [300.0, 300.0, 400.0]);
    assert_eq!(of_kind(&document, &DiagnosticKind::WidthsOverlap).len(), 1);
}

#[test]
fn a_malformed_width_array_is_reported() {
    let (document, font) = fonts(&type0(
        "/Identity-H",
        "CIDFontType2",
        "Identity",
        "/W [1 [500] 5 /x 6]",
    ));
    // The group before the fault stands.
    assert_eq!(widths(&font, &[0, 1]), [500.0]);
    assert_eq!(of_kind(&document, &DiagnosticKind::WidthInvalid).len(), 1);
}

/// 9.7.4.3: a vertical font's default position vector is (w0 / 2, 880) and
/// its displacement (0, -1000); `DW2` and `W2` change them.
#[test]
fn vertical_metrics_default_and_by_cid() {
    let (_, font) = fonts(&type0(
        "/Identity-V",
        "CIDFontType2",
        "Identity",
        "/W [2 [600]] /W2 [3 [-900 250 700] 5 6 -800 100 200]",
    ));
    assert!(font.is_vertical());
    let chars: Vec<_> = font.chars(&[0, 2, 0, 3, 0, 6, 0, 7]).collect();
    let metrics: Vec<_> = chars
        .iter()
        .map(|c| c.vertical.expect("vertical"))
        .collect();
    // CID 2: defaults, with the width 600 halved; CID 3: W2; CID 6: the
    // range of W2; CID 7: defaults over the width 1000.
    assert_eq!(
        (metrics[0].advance, metrics[0].origin_x, metrics[0].origin_y),
        (-1000.0, 300.0, 880.0)
    );
    assert_eq!(
        (metrics[1].advance, metrics[1].origin_x, metrics[1].origin_y),
        (-900.0, 250.0, 700.0)
    );
    assert_eq!(
        (metrics[2].advance, metrics[2].origin_x, metrics[2].origin_y),
        (-800.0, 100.0, 200.0)
    );
    assert_eq!(
        (metrics[3].advance, metrics[3].origin_x, metrics[3].origin_y),
        (-1000.0, 500.0, 880.0)
    );
}

#[test]
fn dw2_changes_the_default_vertical_metrics() {
    let (_, font) = fonts(&type0(
        "/Identity-V",
        "CIDFontType2",
        "Identity",
        "/DW2 [800 -900]",
    ));
    let vertical = font.chars(&[0, 1]).next().unwrap().vertical.unwrap();
    assert_eq!((vertical.advance, vertical.origin_y), (-900.0, 800.0));
}

/// 9.10.2: the CID of a font of the Adobe-Japan1 collection is mapped by
/// Adobe-Japan1-UCS2 to Unicode: CID 1 is the space, 34 is A.
#[test]
fn the_text_of_a_cid_comes_from_its_character_collection() {
    let (_, font) = fonts(&type0("/Identity-H", "CIDFontType0", "Japan1", ""));
    assert_eq!(
        texts(&font, &[0, 1, 0, 34, 0, 0]),
        [Some(" ".into()), Some("A".into()), None]
    );
    assert_eq!(
        font.system_info().map(|i| i.ordering.as_str()),
        Some("Japan1")
    );
    assert_eq!(font.system_info().map(|i| i.supplement), Some(2));
}

#[test]
fn a_collection_with_no_unicode_map_has_no_text() {
    let (_, font) = fonts(&type0("/Identity-H", "CIDFontType0", "Identity", ""));
    assert_eq!(texts(&font, &[0, 34]), [None]);
}

/// A Unicode CMap's codes are the text: UniJIS-UCS2-H maps U+3042 to CID
/// 843 (its range <3041> <3093> starts at CID 842).
#[test]
fn a_unicode_cmap_gives_its_codes_as_text() {
    let (_, font) = fonts(&type0("/UniJIS-UCS2-H", "CIDFontType0", "Japan1", ""));
    let chars: Vec<_> = font.chars(&[0x30, 0x42, 0x00, 0x41]).collect();
    assert_eq!((chars[0].code, chars[0].cid), (0x3042, 843));
    assert_eq!(chars[0].unicode.as_deref(), Some("\u{3042}"));
    assert_eq!(chars[1].unicode.as_deref(), Some("A"));
}

/// UTF-16 CMaps have four-byte codes for a surrogate pair:
/// D840 DC0B is U+2000B.
#[test]
fn a_utf16_cmap_reads_surrogate_pairs() {
    let (_, font) = fonts(&type0("/UniJIS-UTF16-H", "CIDFontType0", "Japan1", ""));
    let chars: Vec<_> = font.chars(&[0xD8, 0x40, 0xDC, 0x0B, 0x30, 0x42]).collect();
    assert_eq!((chars[0].code, chars[0].len), (0xD840_DC0B, 4));
    assert_eq!(chars[0].unicode.as_deref(), Some("\u{2000B}"));
    assert_eq!((chars[1].code, chars[1].len), (0x3042, 2));
    assert_eq!(chars[1].unicode.as_deref(), Some("\u{3042}"));
}

/// 9.7.6.2: 90ms-RKSJ-H has one-byte and two-byte codes. 0x41 is CID 264
/// (its range <20> <7d> starts at 231), 0x8140 is 633 and 0xB1 is 343 (its
/// range <a0> <df> starts at 326); Adobe-Japan1-UCS2 gives them A, U+3000
/// and U+FF71.
#[test]
fn shift_jis_codes_have_mixed_lengths() {
    let (_, font) = fonts(&type0("/90ms-RKSJ-H", "CIDFontType0", "Japan1", ""));
    let chars: Vec<_> = font.chars(&[0x41, 0x81, 0x40, 0xB1]).collect();
    let got: Vec<_> = chars.iter().map(|c| (c.code, c.len, c.cid)).collect();
    assert_eq!(got, [(0x41, 1, 264), (0x8140, 2, 633), (0xB1, 1, 343)]);
    let text: Vec<_> = chars.iter().map(|c| c.unicode.as_deref()).collect();
    assert_eq!(text, [Some("A"), Some("\u{3000}"), Some("\u{FF71}")]);
    // The space byte is a space for word spacing only as a one-byte code.
    assert!(font.chars(&[0x20]).next().unwrap().is_space);
    assert!(!font.chars(&[0x00, 0x20]).next().unwrap().is_space);
}

/// 9.7.6.3: an invalid code takes the bytes of the best partial match and
/// maps to the notdef CID.
#[test]
fn an_invalid_code_takes_the_partial_match_and_maps_to_notdef() {
    let (_, font) = fonts(&type0("/90ms-RKSJ-H", "CIDFontType0", "Japan1", ""));
    let chars: Vec<_> = font.chars(&[0x81, 0x30, 0x41]).collect();
    assert_eq!((chars[0].code, chars[0].len, chars[0].cid), (0x8130, 2, 0));
    assert_eq!((chars[1].code, chars[1].len), (0x41, 1));
}

#[test]
fn a_string_ending_inside_a_code_ends_the_last_character() {
    let (_, font) = fonts(&type0("/Identity-H", "CIDFontType2", "Identity", ""));
    let chars: Vec<_> = font.chars(&[0, 1, 0]).collect();
    assert_eq!(chars.len(), 2);
    assert_eq!(chars[1].len, 1);
}

#[test]
fn an_unknown_cmap_name_is_refused() {
    let objects = type0("/No-Such-CMap", "CIDFontType2", "Identity", "");
    let borrowed: Vec<(u32, &[u8])> = objects.iter().map(|(n, b)| (*n, b.as_slice())).collect();
    let document = font_document(&borrowed).open();
    assert!(matches!(
        load(&document, 60),
        Err(PdfError::UnknownCMap { name }) if name == "No-Such-CMap"
    ));
}

#[test]
fn the_cid_to_gid_map_of_a_type2_font_is_read() {
    let mut objects = type0(
        "/Identity-H",
        "CIDFontType2",
        "Identity",
        "/CIDToGIDMap 63 0 R",
    );
    objects.push((63, stream("", &[0, 0, 0, 7, 1, 2])));
    let (document, font) = fonts(&objects);
    assert_eq!(
        font.cid_to_gid(&document).unwrap(),
        Some(vec![0, 7, 0x0102])
    );
    let mut identity = type0(
        "/Identity-H",
        "CIDFontType2",
        "Identity",
        "/CIDToGIDMap /Identity",
    );
    identity.push((63, stream("", b"")));
    let (document, font) = fonts(&identity);
    assert_eq!(font.cid_to_gid(&document).unwrap(), None);
}

#[test]
fn a_cid_to_gid_map_that_is_no_stream_is_reported() {
    let (document, font) = fonts(&type0(
        "/Identity-H",
        "CIDFontType2",
        "Identity",
        "/CIDToGIDMap 62 0 R",
    ));
    assert_eq!(font.cid_to_gid(&document).unwrap(), None);
    assert_eq!(
        of_kind(&document, &DiagnosticKind::CidToGidUnreadable).len(),
        1
    );
}

#[test]
fn the_descendants_of_a_type0_font_are_checked() {
    let mut objects = type0("/Identity-H", "CIDFontType2", "Identity", "");
    objects[0].1 = b"<< /Type /Font /Subtype /Type0 /BaseFont /Foo /Encoding /Identity-H \
                     /DescendantFonts [61 0 R 61 0 R] >>"
        .to_vec();
    let (document, _) = fonts(&objects);
    assert_eq!(
        of_kind(&document, &DiagnosticKind::DescendantFontCount).len(),
        1
    );

    let mut bad = type0("/Identity-H", "CIDFontType2", "Identity", "");
    bad[1].1 = b"<< /Type /Font /Subtype /Type1 >>".to_vec();
    let borrowed: Vec<(u32, &[u8])> = bad.iter().map(|(n, b)| (*n, b.as_slice())).collect();
    let document = font_document(&borrowed).open();
    assert!(matches!(
        load(&document, 60),
        Err(PdfError::InvalidEntry {
            key: "DescendantFonts",
            ..
        })
    ));
}

#[test]
fn the_descendants_descriptor_is_the_fonts() {
    let (_, font) = fonts(&type0("/Identity-H", "CIDFontType2", "Identity", ""));
    assert_eq!(
        font.descriptor().and_then(|d| d.font_name.as_deref()),
        Some("Foo")
    );
    assert!(font.descriptor().is_some_and(|d| d.flags.symbolic()));
}

/// 9.10.3: ToUnicode on a composite font: a range with a destination of
/// two characters, a range of an array, and a code outside them.
#[test]
fn to_unicode_of_a_composite_font() {
    let mut objects = type0("/Identity-H", "CIDFontType2", "Identity", "");
    objects[0].1 = b"<< /Type /Font /Subtype /Type0 /BaseFont /Foo /Encoding /Identity-H \
                     /DescendantFonts [61 0 R] /ToUnicode 63 0 R >>"
        .to_vec();
    objects.push((
        63,
        to_unicode(
            "1 beginbfchar <0003> <00660069> endbfchar\n\
             2 beginbfrange <0010> <0012> [<0041> <0042> <0043>]\n\
             <0020> <0022> <0061> endbfrange",
        ),
    ));
    let (_, font) = fonts(&objects);
    assert_eq!(
        texts(&font, &[0, 3, 0, 0x11, 0, 0x22, 0, 0x30]),
        [Some("fi".into()), Some("B".into()), Some("c".into()), None]
    );
}
