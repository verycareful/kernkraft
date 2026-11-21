// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Simple fonts (`simple.rs`, ISO 32000-1:2008, 9.6).

use super::*;
use kernkraft_pdf::font::FontKind;

/// A font dictionary with `entries`.
fn font(subtype: &str, entries: &str) -> Vec<u8> {
    format!("<< /Type /Font /Subtype /{subtype} {entries} >>").into_bytes()
}

/// 9.6.2.2: a standard font needs no `Widths`; Adobe's metrics give its
/// widths. Helvetica's A is 667 and its space 278 (its AFM file).
#[test]
fn a_standard_font_without_widths_has_the_afm_widths() {
    let (document, font) = font_of(
        &[(
            60,
            &font("Type1", "/BaseFont /Helvetica /Encoding /WinAnsiEncoding"),
        )],
        60,
    );
    let chars: Vec<_> = font.chars(b"A ").collect();
    assert_eq!(chars.len(), 2);
    assert_eq!((chars[0].code, chars[0].len, chars[0].cid), (65, 1, 65));
    assert_eq!(chars[0].glyph_name, Some("A"));
    assert_eq!(chars[0].width, 667.0);
    assert_eq!(chars[1].glyph_name, Some("space"));
    assert_eq!(chars[1].width, 278.0);
    assert!(chars[1].is_space && !chars[0].is_space);
    assert_eq!(texts(&font, b"A "), [Some("A".into()), Some(" ".into())]);
    assert_eq!(font.kind(), FontKind::Type1);
    assert_eq!(font.base_font(), Some("Helvetica"));
    assert!(document.diagnostics().is_empty());
}

/// Arial and Helvetica were drawn to the same metrics: a file without
/// `Widths` that names Arial means Helvetica's, and a subset prefix is
/// dropped before the name is read.
#[test]
fn arial_without_widths_has_helvetica_widths() {
    for name in ["Arial", "ArialMT", "ABCDEF+ArialMT"] {
        let (_, font) = font_of(
            &[(
                60,
                &font(
                    "TrueType",
                    &format!("/BaseFont /{name} /Encoding /WinAnsiEncoding"),
                ),
            )],
            60,
        );
        assert_eq!(widths(&font, b"A "), [667.0, 278.0], "{name}");
    }
}

/// Bold has its own widths: Helvetica-Bold's A is 722 (its AFM file).
#[test]
fn the_style_of_an_alias_picks_the_metrics() {
    let (_, font) = font_of(
        &[(
            60,
            &font(
                "TrueType",
                "/BaseFont /Arial,Bold /Encoding /WinAnsiEncoding",
            ),
        )],
        60,
    );
    assert_eq!(widths(&font, b"A"), [722.0]);
}

/// The metrics file has no Euro: the width is the missing width.
#[test]
fn a_glyph_the_metrics_lack_has_the_missing_width() {
    let (_, font) = font_of(
        &[(
            60,
            &font("Type1", "/BaseFont /Helvetica /Encoding /WinAnsiEncoding"),
        )],
        60,
    );
    let euro: Vec<_> = font.chars(&[0x80]).collect();
    assert_eq!(euro[0].glyph_name, Some("Euro"));
    assert_eq!(euro[0].unicode.as_deref(), Some("\u{20AC}"));
    assert_eq!(euro[0].width, 0.0);
}

/// Table 111: `Widths` starts at `FirstChar`; a code outside it has the
/// descriptor's `MissingWidth`, or 0 without a descriptor.
#[test]
fn widths_start_at_first_char_and_the_rest_are_missing() {
    let objects: &[(u32, &[u8])] = &[
        (
            60,
            &font(
                "TrueType",
                "/BaseFont /Foo /FirstChar 65 /LastChar 67 /Widths [500 600 700.5] \
                 /FontDescriptor 61 0 R /Encoding /WinAnsiEncoding",
            ),
        ),
        (
            61,
            b"<< /Type /FontDescriptor /FontName /Foo /Flags 32 /MissingWidth 250 >>",
        ),
        (
            62,
            &font(
                "TrueType",
                "/BaseFont /Foo /FirstChar 65 /LastChar 66 /Widths [500 600]",
            ),
        ),
    ];
    let (document, font) = font_of(objects, 60);
    assert_eq!(
        widths(&font, &[64, 65, 66, 67, 68]),
        [250.0, 500.0, 600.0, 700.5, 250.0]
    );
    assert_eq!(font.descriptor().map(|d| d.missing_width), Some(250.0));
    assert!(document.diagnostics().is_empty());
    let without = load(&document, 62).unwrap();
    assert_eq!(widths(&without, &[64, 65, 67]), [0.0, 500.0, 0.0]);
}

#[test]
fn a_font_with_no_widths_is_reported_once() {
    let (document, _) = font_of(&[(60, &font("Type1", "/BaseFont /Foo"))], 60);
    assert_eq!(of_kind(&document, &DiagnosticKind::WidthsMissing).len(), 1);
}

#[test]
fn a_short_widths_array_and_a_width_that_is_no_number_are_reported() {
    let (document, font) = font_of(
        &[(
            60,
            &font(
                "Type1",
                "/BaseFont /Foo /FirstChar 65 /LastChar 70 /Widths [500 /x 700]",
            ),
        )],
        60,
    );
    assert_eq!(widths(&font, &[65, 66, 67, 68]), [500.0, 0.0, 700.0, 0.0]);
    assert_eq!(of_kind(&document, &DiagnosticKind::WidthsShort).len(), 1);
    assert_eq!(of_kind(&document, &DiagnosticKind::WidthInvalid).len(), 1);
}

/// 9.6.6.1, Table 114: `Differences` renames codes from a base; each name
/// after a code is for the next code.
#[test]
fn differences_rename_codes_of_the_base_encoding() {
    let (document, font) = font_of(
        &[
            (
                60,
                &font(
                    "Type1",
                    "/BaseFont /Foo /Encoding 61 0 R /FirstChar 0 /Widths []",
                ),
            ),
            (
                61,
                b"<< /Type /Encoding /BaseEncoding /WinAnsiEncoding \
                  /Differences [65 /Euro /bullet 200 /fi] >>",
            ),
        ],
        60,
    );
    assert_eq!(font.glyph_name(65), Some("Euro"));
    assert_eq!(font.glyph_name(66), Some("bullet"));
    assert_eq!(font.glyph_name(67), Some("C"));
    assert_eq!(font.glyph_name(200), Some("fi"));
    assert_eq!(
        texts(&font, &[65, 66, 67, 200]),
        [
            Some("\u{20AC}".into()),
            Some("\u{2022}".into()),
            Some("C".into()),
            Some("\u{FB01}".into())
        ]
    );
    assert!(of_kind(&document, &DiagnosticKind::EncodingDifferenceInvalid).is_empty());
}

#[test]
fn a_difference_that_does_not_belong_is_reported_and_skipped() {
    let (document, font) = font_of(
        &[
            (60, &font("Type1", "/BaseFont /Foo /Encoding 61 0 R")),
            (61, b"<< /Differences [/orphan 300 /x 65 /Z (s) /Y] >>"),
        ],
        60,
    );
    // The orphan name has no code, 300 is no code, so /x has none; then 65
    // is Z and the string is skipped, so /Y is the code after Z. The four
    // faults are one diagnostic, which a font reports once.
    assert_eq!(font.glyph_name(65), Some("Z"));
    assert_eq!(font.glyph_name(66), Some("Y"));
    assert_eq!(
        of_kind(&document, &DiagnosticKind::EncodingDifferenceInvalid).len(),
        1
    );
}

/// 9.6.6.1: without an `Encoding`, a nonsymbolic font has StandardEncoding,
/// in which 047 is quoteright.
#[test]
fn a_font_with_no_encoding_has_the_standard_one() {
    let (_, font) = font_of(
        &[(60, &font("Type1", "/BaseFont /Foo /FirstChar 0 /Widths []"))],
        60,
    );
    assert_eq!(font.glyph_name(0o47), Some("quoteright"));
    assert_eq!(texts(&font, &[0o47]), [Some("\u{2019}".into())]);
}

/// 9.6.2.2: Symbol and ZapfDingbats have their own built-in encodings
/// (Annex D.5, D.6); Alpha (0101) is 722 wide in Symbol, a1 (041) is 974 in
/// ZapfDingbats.
#[test]
fn symbol_and_zapf_dingbats_have_their_own_encodings() {
    let (_, symbol) = font_of(&[(60, &font("Type1", "/BaseFont /Symbol"))], 60);
    assert_eq!(symbol.glyph_name(0o101), Some("Alpha"));
    assert_eq!(texts(&symbol, &[0o101]), [Some("\u{0391}".into())]);
    assert_eq!(widths(&symbol, &[0o101]), [722.0]);
    let (_, zapf) = font_of(&[(60, &font("Type1", "/BaseFont /ZapfDingbats"))], 60);
    assert_eq!(zapf.glyph_name(0o41), Some("a1"));
    assert_eq!(widths(&zapf, &[0o41]), [974.0]);
}

#[test]
fn an_unknown_encoding_name_is_reported_and_the_standard_one_used() {
    let (document, font) = font_of(
        &[(
            60,
            &font(
                "Type1",
                "/BaseFont /Foo /Encoding /MyEncoding /FirstChar 0 /Widths []",
            ),
        )],
        60,
    );
    assert_eq!(font.glyph_name(65), Some("A"));
    let found = of_kind(
        &document,
        &DiagnosticKind::EncodingUnknown {
            name: kernkraft_pdf::syntax::Name::new("MyEncoding"),
        },
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].object, Some(id(60)));
}

/// 9.10.3: a ToUnicode CMap gives the text, before the glyph name does.
#[test]
fn to_unicode_overrides_the_glyph_name() {
    let (_, font) = font_of(
        &[
            (
                60,
                &font(
                    "Type1",
                    "/BaseFont /Foo /Encoding /WinAnsiEncoding /ToUnicode 61 0 R",
                ),
            ),
            (
                61,
                &to_unicode(
                    "1 beginbfchar <0041> <0058> endbfchar\n\
                     1 beginbfrange <0042> <0043> <0059> endbfrange",
                ),
            ),
        ],
        60,
    );
    // A is X, B and C are Y and Z, D falls to its glyph name.
    assert_eq!(
        texts(&font, b"ABCD"),
        [
            Some("X".into()),
            Some("Y".into()),
            Some("Z".into()),
            Some("D".into())
        ]
    );
}

/// 9.6.6.4: a TrueType font reaches glyphs through the program's `cmap`
/// unless its Encoding names MacRoman or WinAnsi or it is nonsymbolic.
#[test]
fn a_symbolic_truetype_font_uses_its_cmap_directly() {
    let objects: &[(u32, &[u8])] = &[
        (
            60,
            &font("TrueType", "/BaseFont /Foo /FontDescriptor 61 0 R"),
        ),
        (61, b"<< /Type /FontDescriptor /FontName /Foo /Flags 4 >>"),
        (
            62,
            &font(
                "TrueType",
                "/BaseFont /Foo /FontDescriptor 61 0 R /Encoding /WinAnsiEncoding",
            ),
        ),
        (
            63,
            &font("TrueType", "/BaseFont /Foo /FontDescriptor 64 0 R"),
        ),
        (64, b"<< /Type /FontDescriptor /FontName /Foo /Flags 32 >>"),
        (65, &font("Type1", "/BaseFont /Foo /FontDescriptor 61 0 R")),
    ];
    let document = font_document(objects).open();
    assert!(load(&document, 60).unwrap().truetype_uses_cmap_directly());
    assert!(!load(&document, 62).unwrap().truetype_uses_cmap_directly());
    assert!(!load(&document, 63).unwrap().truetype_uses_cmap_directly());
    assert!(!load(&document, 65).unwrap().truetype_uses_cmap_directly());
}

/// 9.6.5, Table 113: a Type 3 font's names come from its `Encoding` alone
/// and its widths are in glyph space: 50 at a `FontMatrix` of 0.01 is half
/// an em.
#[test]
fn a_type3_font_scales_widths_by_its_matrix() {
    let (_, font) = font_of(
        &[
            (
                60,
                &font(
                    "Type3",
                    "/FontBBox [0 0 100 100] /FontMatrix [0.01 0 0 0.01 0 0] \
                     /CharProcs << /a 61 0 R /b 62 0 R >> \
                     /Encoding << /Type /Encoding /Differences [97 /a /b] >> \
                     /FirstChar 97 /LastChar 98 /Widths [50 80]",
                ),
            ),
            (61, &stream("", b"50 0 d0")),
            (62, &stream("", b"80 0 d0")),
        ],
        60,
    );
    assert_eq!(font.kind(), FontKind::Type3);
    assert_eq!(widths(&font, b"ab"), [500.0, 800.0]);
    assert_eq!(font.glyph_name(97), Some("a"));
    let type3 = font.type3().expect("a Type 3 font");
    assert_eq!(type3.font_matrix, [0.01, 0.0, 0.0, 0.01, 0.0, 0.0]);
    assert_eq!(type3.font_bbox, Some([0.0, 0.0, 100.0, 100.0]));
    assert_eq!(type3.char_procs.get("a"), Some(&id(61)));
    assert_eq!(type3.char_procs.get("b"), Some(&id(62)));
    // 97 has no name unless the Encoding gives it: code 99 has none.
    assert_eq!(font.glyph_name(99), None);
}

#[test]
fn a_type3_font_without_char_procs_is_refused() {
    let document = font_document(&[(60, &font("Type3", "/FontBBox [0 0 1 1]"))]).open();
    assert!(matches!(
        load(&document, 60),
        Err(PdfError::InvalidEntry {
            key: "CharProcs",
            ..
        })
    ));
}

#[test]
fn a_dictionary_that_is_no_font_is_refused() {
    let document = font_document(&[
        (60, b"<< /Type /Font /Subtype /Other >>"),
        (61, b"<< /Type /Font >>"),
        (62, b"[1 2]"),
    ])
    .open();
    for number in [60, 61] {
        assert!(matches!(
            load(&document, number),
            Err(PdfError::InvalidEntry { key: "Subtype", .. })
        ));
    }
    assert!(matches!(
        load(&document, 62),
        Err(PdfError::InvalidEntry { key: "Font", .. })
    ));
}

/// 9.6.6.2: an embedded Type 1 program's own `Encoding` is the base of a
/// font whose dictionary gives none. The program here puts Euro at 65 and
/// bullet at 66.
#[test]
fn an_embedded_type1_program_gives_the_built_in_encoding() {
    let program = type1::type1(
        &[".notdef", "Euro", "bullet"],
        &[(65, "Euro"), (66, "bullet")],
    );
    let objects: &[(u32, &[u8])] = &[
        (
            60,
            &font(
                "Type1",
                "/BaseFont /Test /FontDescriptor 61 0 R /FirstChar 65 /LastChar 66 /Widths [500 600]",
            ),
        ),
        (
            61,
            b"<< /Type /FontDescriptor /FontName /Test /Flags 4 /MissingWidth 300 /FontFile 62 0 R >>",
        ),
        (62, &stream("", &program)),
    ];
    let (document, font) = font_of(objects, 60);
    assert_eq!(font.glyph_name(65), Some("Euro"));
    assert_eq!(font.glyph_name(66), Some("bullet"));
    assert_eq!(font.glyph_name(67), None);
    assert_eq!(
        texts(&font, &[65, 66, 67]),
        [Some("\u{20AC}".into()), Some("\u{2022}".into()), None]
    );
    assert_eq!(widths(&font, &[65, 66, 67]), [500.0, 600.0, 300.0]);
    let embedded = font
        .embedded_program(&document)
        .unwrap()
        .expect("a program");
    assert_eq!(embedded.kind, kernkraft_pdf::font::ProgramKind::Type1);
    assert_eq!(embedded.data, program);
    assert!(document.diagnostics().is_empty());
}

/// Differences are applied to the program's own encoding when the
/// dictionary names no base.
#[test]
fn differences_apply_to_the_programs_encoding() {
    let program = type1::type1(&[".notdef", "Euro", "bullet"], &[(65, "Euro")]);
    let objects: &[(u32, &[u8])] = &[
        (
            60,
            &font(
                "Type1",
                "/BaseFont /Test /FontDescriptor 61 0 R /Encoding 63 0 R",
            ),
        ),
        (
            61,
            b"<< /Type /FontDescriptor /FontName /Test /Flags 4 /FontFile 62 0 R >>",
        ),
        (62, &stream("", &program)),
        (63, b"<< /Differences [66 /bullet] >>"),
    ];
    let (_, font) = font_of(objects, 60);
    assert_eq!(font.glyph_name(65), Some("Euro"));
    assert_eq!(font.glyph_name(66), Some("bullet"));
}

#[test]
fn an_unreadable_font_program_is_reported() {
    let objects: &[(u32, &[u8])] = &[
        (
            60,
            &font(
                "Type1",
                "/BaseFont /Test /FontDescriptor 61 0 R /Encoding 63 0 R",
            ),
        ),
        (
            61,
            b"<< /Type /FontDescriptor /FontName /Test /Flags 4 /FontFile 62 0 R >>",
        ),
        (62, &stream("", b"this is not a font")),
        (63, b"<< /Differences [66 /bullet] >>"),
    ];
    let (document, font) = font_of(objects, 60);
    assert_eq!(font.glyph_name(66), Some("bullet"));
    assert_eq!(font.glyph_name(65), None);
    assert_eq!(
        of_kind(&document, &DiagnosticKind::FontProgramUnreadable).len(),
        1
    );
}

#[test]
fn flags_and_the_rest_of_the_descriptor_are_read() {
    let (_, font) = font_of(
        &[
            (
                60,
                &font("TrueType", "/BaseFont /Foo /FontDescriptor 61 0 R"),
            ),
            (
                61,
                b"<< /Type /FontDescriptor /FontName /Foo /FontFamily (Foo Sans) /Flags 34 \
                  /FontBBox [-10 -20 1000 900] /ItalicAngle -12 /Ascent 800 /Descent -200 \
                  /CapHeight 700 /XHeight 500 /StemV 80 /AvgWidth 450 /MaxWidth 1100 >>",
            ),
        ],
        60,
    );
    let descriptor = font.descriptor().expect("a descriptor");
    assert_eq!(descriptor.font_family.as_deref(), Some("Foo Sans"));
    assert!(descriptor.flags.nonsymbolic() && descriptor.flags.serif());
    assert_eq!(descriptor.font_bbox, Some([-10.0, -20.0, 1000.0, 900.0]));
    assert_eq!(descriptor.italic_angle, -12.0);
    assert_eq!((descriptor.ascent, descriptor.descent), (800.0, -200.0));
    assert_eq!(
        (descriptor.cap_height, descriptor.x_height),
        (Some(700.0), Some(500.0))
    );
    assert_eq!(
        (
            descriptor.stem_v,
            descriptor.avg_width,
            descriptor.max_width
        ),
        (80.0, 450.0, 1100.0)
    );
}
