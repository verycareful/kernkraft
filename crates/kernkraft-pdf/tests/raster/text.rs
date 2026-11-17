// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Text: ISO 32000-1, 9.3.6 (rendering modes), 9.4 (showing text), 9.6.5
//! (Type 3 fonts), 9.6.6 (encodings of simple fonts), 9.7.4.2 (CID glyph
//! selection) and 9.6.2.2 (fonts that are not embedded).
//!
//! The fonts are written table by table. Glyph `A` of the TrueType font is
//! a rectangle 500 by 700 units on an em of 1000 units, and every width
//! is 600, so at size 20 a letter covers 10 by 14 points and the pen moves
//! 12.

#[path = "../../../kernkraft-font/tests/common/mod.rs"]
mod fontkit;

use fontkit::sfnt::{self, Glyph, SfntSpec};
use kernkraft_font::FontList;

use super::*;

const RECTANGLE: [(i16, i16, bool); 4] = [
    (0, 0, true),
    (500, 0, true),
    (500, 700, true),
    (0, 700, true),
];

fn glyph(name: &'static str, contours: Vec<Vec<(i16, i16, bool)>>) -> Glyph {
    Glyph {
        name,
        advance: 600,
        contours,
    }
}

/// A TrueType program: glyph 1 is `A`, a rectangle; glyph 2 is `B`, a
/// small square 200 by 200 at the origin; both reached from Unicode.
pub(super) fn truetype(family: &'static str) -> Vec<u8> {
    let mut spec = SfntSpec::new(vec![
        glyph(".notdef", vec![]),
        glyph("A", vec![RECTANGLE.to_vec()]),
        glyph(
            "B",
            vec![vec![
                (0, 0, true),
                (200, 0, true),
                (200, 200, true),
                (0, 200, true),
            ]],
        ),
    ]);
    spec.unicode = vec![(0x41, 1), (0x42, 2)];
    spec.mac = vec![(0x41, 1), (0x42, 2)];
    spec.family = family;
    sfnt::build(&spec)
}

/// The same program with only a Macintosh `cmap`: what a symbolic font
/// reaches its glyphs through.
fn symbolic_truetype() -> Vec<u8> {
    let mut spec = SfntSpec::new(vec![
        glyph(".notdef", vec![]),
        glyph("A", vec![RECTANGLE.to_vec()]),
    ]);
    spec.mac = vec![(0x41, 1)];
    sfnt::build(&spec)
}

/// The objects of a simple TrueType font at object 20, its descriptor at 21
/// and its program at 22.
pub(super) fn truetype_font(font_entries: &str, flags: u32, program: &[u8]) -> Vec<(u32, Vec<u8>)> {
    vec![
        (
            20,
            format!(
                "<< /Type /Font /Subtype /TrueType /BaseFont /AAAAAA+Test /FirstChar 65 \
                 /LastChar 67 /Widths [600 600 600] /FontDescriptor 21 0 R {font_entries} >>"
            )
            .into_bytes(),
        ),
        (
            21,
            format!(
                "<< /Type /FontDescriptor /FontName /AAAAAA+Test /Flags {flags} \
                 /FontBBox [0 0 600 700] /ItalicAngle 0 /Ascent 800 /Descent -200 \
                 /CapHeight 700 /StemV 80 /FontFile2 22 0 R >>"
            )
            .into_bytes(),
        ),
        (22, stream(&format!(" /Length1 {}", program.len()), program)),
    ]
}

fn page_with(content: &str, objects: &[(u32, Vec<u8>)]) -> Document {
    square_page(
        content,
        "/Font << /F1 20 0 R >>",
        &objects
            .iter()
            .map(|(n, b)| (*n, b.as_slice()))
            .collect::<Vec<_>>(),
    )
}

fn simple_page(content: &str) -> Document {
    let objects = truetype_font("/Encoding /WinAnsiEncoding", 32, &truetype("Test Sans"));
    page_with(content, &objects)
}

#[test]
fn a_glyph_of_an_embedded_truetype_font_is_filled() {
    // Size 20 at (2, 2): the rectangle is 10 by 14, user x 2 to 12 and
    // y 2 to 16, rows 4 to 18.
    let raster = render(&simple_page("BT /F1 20 Tf 2 2 Td (A) Tj ET"));
    assert_box(&raster, 2, 4, 12, 18, BLACK);
    assert_box(&raster, 0, 0, SIDE, 4, WHITE);
    assert_box(&raster, 12, 0, SIDE, SIDE, WHITE);
    assert_box(&raster, 0, 18, SIDE, SIDE, WHITE);
}

#[test]
fn the_pen_moves_by_the_width_of_each_glyph() {
    // Size 10: a letter is 5 by 7 points and the pen moves 6.
    let raster = render(&simple_page("BT /F1 10 Tf 1 2 Td (AA) Tj ET"));
    assert_box(&raster, 1, 11, 6, 18, BLACK);
    assert_box(&raster, 7, 11, 12, 18, BLACK);
    assert_box(&raster, 6, 11, 7, 18, WHITE);
}

#[test]
fn text_is_painted_in_the_fill_colour() {
    let raster = render(&simple_page("1 0 0 rg BT /F1 20 Tf 2 2 Td (A) Tj ET"));
    assert_box(&raster, 2, 4, 12, 18, RED);
}

#[test]
fn an_invisible_glyph_leaves_no_mark() {
    let raster = render(&simple_page("BT /F1 20 Tf 3 Tr 2 2 Td (A) Tj ET"));
    assert_box(&raster, 0, 0, SIDE, SIDE, WHITE);
}

#[test]
fn stroked_text_draws_the_outline_only() {
    // Mode 1, line width 2: a band of 1 point each side of the edge. The
    // middle of the rectangle stays white, its edge is dark.
    let raster = render(&simple_page("BT /F1 20 Tf 1 Tr 2 w 2 2 Td (A) Tj ET"));
    assert_eq!(px(&raster, 7, 11), WHITE);
    assert_eq!(px(&raster, 2, 11), BLACK);
    assert_eq!(px(&raster, 11, 11), BLACK);
}

#[test]
fn fill_then_stroke_uses_both_colours() {
    let raster = render(&simple_page(
        "1 0 0 rg 0 0 1 RG BT /F1 20 Tf 2 Tr 2 w 2 2 Td (A) Tj ET",
    ));
    assert_eq!(px(&raster, 7, 11), RED);
    assert_eq!(px(&raster, 2, 11), BLUE);
}

#[test]
fn text_in_a_clip_mode_cuts_what_follows_at_the_end_of_the_text_object() {
    // Mode 7 adds the glyph to the clip; the fill after ET is cut to it.
    let raster = render(&simple_page(
        "BT /F1 20 Tf 7 Tr 2 2 Td (A) Tj ET 0 0 1 rg 0 0 20 20 re f",
    ));
    assert_box(&raster, 2, 4, 12, 18, BLUE);
    assert_box(&raster, 12, 0, SIDE, SIDE, WHITE);
    assert_box(&raster, 0, 0, SIDE, 4, WHITE);
}

#[test]
fn a_clip_mode_that_marks_nothing_clips_everything() {
    // A space has no marks, and the clip it makes is empty.
    let raster = render(&simple_page(
        "BT /F1 20 Tf 7 Tr 2 2 Td ( ) Tj ET 0 0 1 rg 0 0 20 20 re f",
    ));
    assert_box(&raster, 0, 0, SIDE, SIDE, WHITE);
}

#[test]
fn text_scales_with_the_ctm_and_the_text_matrix() {
    let a = render(&simple_page("BT /F1 10 Tf 2 0 0 2 2 2 Tm (A) Tj ET"));
    let b = render(&simple_page("2 0 0 2 0 0 cm BT /F1 10 Tf 1 1 Td (A) Tj ET"));
    assert_eq!(a, b);
    assert_box(&a, 2, 4, 12, 18, BLACK);
}

#[test]
fn horizontal_scaling_stretches_the_glyph() {
    let raster = render(&simple_page("BT /F1 10 Tf 200 Tz 2 2 Td (A) Tj ET"));
    assert_box(&raster, 2, 11, 12, 18, BLACK);
}

#[test]
fn text_rise_lifts_the_glyph() {
    let raster = render(&simple_page("BT /F1 10 Tf 4 Ts 2 2 Td (A) Tj ET"));
    assert_box(&raster, 2, 7, 7, 14, BLACK);
}

#[test]
fn a_character_the_font_lacks_is_reported_once_and_skipped() {
    let doc = simple_page("BT /F1 10 Tf 2 2 Td (CC) Tj ET");
    let raster = render(&doc);
    assert_box(&raster, 0, 0, SIDE, SIDE, WHITE);
    let found = diagnostics_of(&doc, |k| matches!(k, DiagnosticKind::GlyphMissing { .. }));
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn a_symbolic_font_reaches_its_glyphs_through_the_cmap_directly() {
    // Flags 4 (symbolic) and no Encoding: code 65 is looked up in the
    // (1, 0) subtable (9.6.6.4).
    let objects = truetype_font("", 4, &symbolic_truetype());
    let raster = render(&page_with("BT /F1 20 Tf 2 2 Td (A) Tj ET", &objects));
    assert_box(&raster, 2, 4, 12, 18, BLACK);
}

#[test]
fn a_composite_font_selects_glyphs_by_cid_and_cidtogidmap() {
    let program = truetype("Test Sans");
    let objects: Vec<(u32, Vec<u8>)> = vec![
        (
            20,
            b"<< /Type /Font /Subtype /Type0 /BaseFont /Test /Encoding /Identity-H \
              /DescendantFonts [23 0 R] >>"
                .to_vec(),
        ),
        (
            23,
            b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Test \
              /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> \
              /FontDescriptor 21 0 R /DW 600 /CIDToGIDMap 24 0 R >>"
                .to_vec(),
        ),
        (
            21,
            b"<< /Type /FontDescriptor /FontName /Test /Flags 4 /FontBBox [0 0 600 700] \
              /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 \
              /FontFile2 22 0 R >>"
                .to_vec(),
        ),
        (
            22,
            stream(&format!(" /Length1 {}", program.len()), &program),
        ),
        // CID 0 -> glyph 0, CID 1 -> glyph 2 (B), CID 2 -> glyph 1 (A).
        (24, stream("", &[0, 0, 0, 2, 0, 1])),
    ];
    // CID 2 is A: a 10 by 14 rectangle.
    let raster = render(&page_with("BT /F1 20 Tf 2 2 Td <0002> Tj ET", &objects));
    assert_box(&raster, 2, 4, 12, 18, BLACK);
    // CID 1 is B, 4 by 4 points.
    let raster = render(&page_with("BT /F1 20 Tf 2 2 Td <0001> Tj ET", &objects));
    assert_box(&raster, 2, 14, 6, 18, BLACK);
    assert_box(&raster, 2, 4, 12, 14, WHITE);
}

#[test]
fn a_type_1_program_is_drawn_by_glyph_name() {
    // hsbw 50 600, rmoveto 100 200, a 300 square: x 150 to 450, y 200 to
    // 500 on an em of 1000. At size 20 and origin (2, 2) that is x 5 to 11
    // and y 6 to 12, rows 8 to 14.
    use fontkit::{Cs, Type1Spec};
    let empty = Cs::new().with(&[0, 250], "hsbw").op("endchar");
    let square = Cs::new()
        .with(&[50, 600], "hsbw")
        .with(&[100, 200], "rmoveto")
        .with(&[300], "hlineto")
        .with(&[300], "vlineto")
        .with(&[-300], "hlineto")
        .op("closepath")
        .op("endchar");
    let program = Type1Spec::new(vec![(".notdef", empty), ("A", square)]).flat();
    let objects: Vec<(u32, Vec<u8>)> = vec![
        (
            20,
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Test /FirstChar 65 /LastChar 65 \
              /Widths [600] /FontDescriptor 21 0 R /Encoding /WinAnsiEncoding >>"
                .to_vec(),
        ),
        (
            21,
            b"<< /Type /FontDescriptor /FontName /Test /Flags 32 /FontBBox [0 0 600 700] \
              /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 \
              /FontFile 22 0 R >>"
                .to_vec(),
        ),
        (
            22,
            stream(&format!(" /Length1 {}", program.len()), &program),
        ),
    ];
    let raster = render(&page_with("BT /F1 20 Tf 2 2 Td (A) Tj ET", &objects));
    assert_box(&raster, 5, 8, 11, 14, BLACK);
    assert_box(&raster, 0, 0, SIDE, 8, WHITE);
    assert_box(&raster, 11, 0, SIDE, SIDE, WHITE);
}

pub(super) fn type3_fonts(procedures: &[(&str, &str)]) -> Vec<(u32, Vec<u8>)> {
    let names: String = procedures
        .iter()
        .enumerate()
        .map(|(i, (name, _))| format!("/{name} {} 0 R", 30 + i))
        .collect::<Vec<_>>()
        .join(" ");
    let mut objects: Vec<(u32, Vec<u8>)> = vec![(
        20,
        format!(
            "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] \
             /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << {names} >> \
             /Encoding << /Type /Encoding /Differences [97 /a /b] >> \
             /FirstChar 97 /LastChar 98 /Widths [600 600] >>"
        )
        .into_bytes(),
    )];
    for (i, (_, code)) in procedures.iter().enumerate() {
        objects.push((30 + i as u32, stream("", code.as_bytes())));
    }
    objects
}

#[test]
fn a_type_3_glyph_runs_its_description() {
    // d0: a coloured glyph that sets its own colour. Size 20: 500 by 700
    // units is 10 by 14 points.
    let objects = type3_fonts(&[("a", "600 0 d0 0 0 1 rg 0 0 500 700 re f")]);
    let raster = render(&page_with(
        "1 0 0 rg BT /F1 20 Tf 2 2 Td (a) Tj ET",
        &objects,
    ));
    assert_box(&raster, 2, 4, 12, 18, BLUE);
}

#[test]
fn a_d1_glyph_takes_the_colour_of_the_text() {
    let objects = type3_fonts(&[("a", "600 0 0 0 500 700 d1 0 0 1 rg 0 0 500 700 re f")]);
    let raster = render(&page_with(
        "1 0 0 rg BT /F1 20 Tf 2 2 Td (a) Tj ET",
        &objects,
    ));
    assert_box(&raster, 2, 4, 12, 18, RED);
}

#[test]
fn type_3_glyphs_advance_by_the_font_widths() {
    let objects = type3_fonts(&[
        ("a", "600 0 0 0 500 700 d1 0 0 500 700 re f"),
        ("b", "600 0 0 0 500 700 d1 0 0 500 700 re f"),
    ]);
    let raster = render(&page_with("BT /F1 10 Tf 1 2 Td (ab) Tj ET", &objects));
    assert_box(&raster, 1, 11, 6, 18, BLACK);
    assert_box(&raster, 7, 11, 12, 18, BLACK);
}

#[test]
fn a_type_3_glyph_cannot_change_the_page_state_around_it() {
    // The description clips and leaves a q open; neither outlasts it.
    let objects = type3_fonts(&[("a", "600 0 d0 q 0 0 1 1 re W n 0 0 500 700 re f")]);
    let raster = render(&page_with(
        "BT /F1 20 Tf 2 2 Td (a) Tj ET 0 0 1 rg 10 0 10 5 re f",
        &objects,
    ));
    assert_box(&raster, 10, 15, 20, 20, BLUE);
}

#[test]
fn d0_outside_a_glyph_is_misplaced() {
    let doc = square_page("600 0 d0", "", &[]);
    render(&doc);
    assert!(!diagnostics_of(&doc, |k| matches!(
        k,
        DiagnosticKind::OperatorMisplaced { .. }
    ))
    .is_empty());
}

#[test]
fn a_font_with_no_program_and_no_list_is_reported_and_not_drawn() {
    let objects: Vec<(u32, Vec<u8>)> = vec![(
        20,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_vec(),
    )];
    let doc = page_with("BT /F1 20 Tf 2 2 Td (A) Tj ET", &objects);
    let raster = render(&doc);
    assert_box(&raster, 0, 0, SIDE, SIDE, WHITE);
    let found = diagnostics_of(&doc, |k| {
        matches!(k, DiagnosticKind::FontUnavailable { .. })
    });
    assert_eq!(found.len(), 1);
}

#[test]
fn a_font_with_no_program_is_drawn_with_a_face_from_the_list() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("helvetica.ttf");
    std::fs::write(&path, truetype("Helvetica")).unwrap();
    let list = FontList::index(&[path], &NoProgress).unwrap();
    assert_eq!(list.faces().len(), 1);
    let objects: Vec<(u32, Vec<u8>)> = vec![(
        20,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_vec(),
    )];
    let doc = page_with("BT /F1 20 Tf 2 2 Td (A) Tj ET", &objects);
    let options = RenderOptions {
        fonts: Some(&list),
        ..Default::default()
    };
    let raster = render_with(&doc, &options).unwrap();
    assert_box(&raster, 2, 4, 12, 18, BLACK);
    let found = diagnostics_of(
        &doc,
        |k| matches!(k, DiagnosticKind::FontSubstituted { family, .. } if family == "Helvetica"),
    );
    assert_eq!(found.len(), 1, "{:?}", doc.diagnostics());
}

#[test]
fn a_damaged_program_falls_back_to_the_list_and_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("helvetica.ttf");
    std::fs::write(&path, truetype("Helvetica")).unwrap();
    let list = FontList::index(&[path], &NoProgress).unwrap();
    let objects = truetype_font("/Encoding /WinAnsiEncoding", 32, b"not a font at all");
    let doc = page_with("BT /F1 20 Tf 2 2 Td (A) Tj ET", &objects);
    let options = RenderOptions {
        fonts: Some(&list),
        ..Default::default()
    };
    let raster = render_with(&doc, &options).unwrap();
    assert_box(&raster, 2, 4, 12, 18, BLACK);
    assert!(
        !diagnostics_of(&doc, |k| matches!(k, DiagnosticKind::FontProgramUnreadable)).is_empty()
    );
}
