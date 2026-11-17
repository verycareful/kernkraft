// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Text: ISO 32000-1, 9.3 (Table 104), 9.4 (Tables 105 to 109) and 9.4.4.
//!
//! Font F1 has the widths 600, 700 and 800 for the codes 65 to 67 (A, B,
//! C); F2 has 250 and 300 for the codes 32 and 33 (space and `!`).

use std::sync::Arc;

use kernkraft_pdf::content::RenderMode;
use kernkraft_pdf::DiagnosticKind;

use super::{assert_matrix, has, name, run, texts, Event, FONTS, FONT_OBJECTS};

fn run_text(content: &str) -> (kernkraft_pdf::doc::Document, Vec<Event>) {
    run(content, FONTS, &FONT_OBJECTS)
}

#[test]
fn glyphs_are_placed_by_the_text_rendering_matrix() {
    // Trm = [Tfs*Th 0 0 Tfs 0 Trise] x Tm x CTM. Tfs 10, Tm moves to
    // (100, 200); the advance of A is 600/1000 * 10 = 6.
    let (document, events) = run_text("BT /F1 10 Tf 100 200 Td (AB) Tj ET");
    let runs = texts(&events);
    assert_eq!(runs.len(), 1);
    let glyphs = &runs[0].glyphs;
    assert_eq!(glyphs.len(), 2);
    assert_matrix(glyphs[0].matrix, [10.0, 0.0, 0.0, 10.0, 100.0, 200.0]);
    assert_matrix(glyphs[1].matrix, [10.0, 0.0, 0.0, 10.0, 106.0, 200.0]);
    assert_eq!(glyphs[0].advance, (6.0, 0.0));
    assert_eq!(glyphs[1].advance, (7.0, 0.0));
    assert_matrix(runs[0].matrix_before, [1.0, 0.0, 0.0, 1.0, 100.0, 200.0]);
    assert_matrix(runs[0].matrix_after, [1.0, 0.0, 0.0, 1.0, 113.0, 200.0]);
    assert!(document.diagnostics().is_empty());
}

#[test]
fn a_glyph_reports_its_character() {
    let (_, events) = run_text("BT /F1 10 Tf (A) Tj ET");
    let glyph = &texts(&events)[0].glyphs[0];
    assert_eq!(glyph.code, 65);
    assert_eq!(glyph.len, 1);
    assert_eq!(glyph.unicode.as_deref(), Some("A"));
    assert_eq!(glyph.glyph_name.as_deref(), Some("A"));
    assert_eq!(glyph.width, 600.0);
    assert!(!glyph.is_space);
    assert_eq!(glyph.operand, 0);
}

#[test]
fn the_run_names_the_font_and_its_size() {
    let (_, events) = run_text("BT /F1 12.5 Tf (A) Tj ET");
    let run = texts(&events)[0];
    assert_eq!(run.font_name, name("F1"));
    assert_eq!(run.font_size, 12.5);
    assert_eq!(run.font.base_font(), Some("Helvetica"));
}

#[test]
fn the_ctm_is_part_of_the_matrix() {
    let (_, events) = run_text("2 0 0 2 0 0 cm BT /F1 10 Tf 100 200 Td (A) Tj ET");
    let glyph = &texts(&events)[0].glyphs[0];
    assert_matrix(glyph.matrix, [20.0, 0.0, 0.0, 20.0, 200.0, 400.0]);
    // The advance is in text space, before the CTM.
    assert_eq!(glyph.advance, (6.0, 0.0));
}

#[test]
fn spacing_and_scaling_apply_as_9_4_4_says() {
    // F2: space 250, `!` 300; Tfs 10, Tc 2, Tw 5, Th 0.5.
    // tx = (w0 * Tfs + Tc + Tw) * Th: the space gets Tw, the `!` does not.
    let (_, events) = run_text("BT /F2 10 Tf 2 Tc 5 Tw 50 Tz ( !) Tj ET");
    let glyphs = &texts(&events)[0].glyphs;
    assert!(glyphs[0].is_space);
    assert_eq!(glyphs[0].advance, ((2.5 + 2.0 + 5.0) * 0.5, 0.0));
    assert_eq!(glyphs[1].advance, ((3.0 + 2.0) * 0.5, 0.0));
    // Horizontal scaling shrinks the glyph: a = Tfs * Th.
    assert_matrix(glyphs[0].matrix, [5.0, 0.0, 0.0, 10.0, 0.0, 0.0]);
    assert_matrix(glyphs[1].matrix, [5.0, 0.0, 0.0, 10.0, 4.75, 0.0]);
}

#[test]
fn rise_lifts_the_glyph() {
    let (_, events) = run_text("BT /F1 10 Tf 3 Ts (A) Tj ET");
    assert_matrix(
        texts(&events)[0].glyphs[0].matrix,
        [10.0, 0.0, 0.0, 10.0, 0.0, 3.0],
    );
}

#[test]
fn tj_array_numbers_move_the_next_string_back() {
    // 1000 thousandths of Tfs 10 is 10 units back; B starts at 6 - 10.
    let (_, events) = run_text("BT /F1 10 Tf [(A) 1000 (B)] TJ ET");
    let run = texts(&events)[0];
    assert_eq!(run.glyphs[0].operand, 0);
    assert_eq!(run.glyphs[1].operand, 2);
    assert_matrix(run.glyphs[1].matrix, [10.0, 0.0, 0.0, 10.0, -4.0, 0.0]);
    assert_eq!(run.adjustments, [(1, -10.0)]);
    assert_matrix(run.matrix_after, [1.0, 0.0, 0.0, 1.0, -4.0 + 7.0, 0.0]);
}

#[test]
fn a_negative_tj_number_opens_a_gap_and_scaling_applies_to_it() {
    let (_, events) = run_text("BT /F1 10 Tf 50 Tz [(A) -500 (B)] TJ ET");
    let run = texts(&events)[0];
    // A advances 6 * 0.5 = 3; the gap is 500/1000 * 10 * 0.5 = 2.5.
    assert_matrix(run.glyphs[1].matrix, [5.0, 0.0, 0.0, 10.0, 5.5, 0.0]);
}

#[test]
fn td_moves_from_the_start_of_the_line() {
    let (_, events) = run_text("BT /F1 10 Tf 10 20 Td 5 5 Td (A) Tj ET");
    assert_matrix(
        texts(&events)[0].glyphs[0].matrix,
        [10.0, 0.0, 0.0, 10.0, 15.0, 25.0],
    );
}

#[test]
fn showing_does_not_move_the_line_matrix() {
    let (_, events) = run_text("BT /F1 10 Tf 10 20 Td (AB) Tj 0 -12 Td (A) Tj ET");
    let runs = texts(&events);
    // The second line starts 12 below the start of the first line, not
    // below the end of the text shown.
    assert_matrix(runs[1].glyphs[0].matrix, [10.0, 0.0, 0.0, 10.0, 10.0, 8.0]);
}

#[test]
fn td_capital_sets_the_leading_and_t_star_uses_it() {
    let (_, events) = run_text("BT /F1 10 Tf 0 -12 TD (A) Tj T* (A) Tj ET");
    let runs = texts(&events);
    assert_matrix(runs[0].glyphs[0].matrix, [10.0, 0.0, 0.0, 10.0, 0.0, -12.0]);
    assert_matrix(runs[1].glyphs[0].matrix, [10.0, 0.0, 0.0, 10.0, 0.0, -24.0]);
    let Event::Text { state, .. } = events
        .iter()
        .find(|e| matches!(e, Event::Text { .. }))
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(state.text.leading, 12.0);
}

#[test]
fn tm_sets_both_matrices() {
    let (_, events) = run_text("BT /F1 10 Tf 2 0 0 2 10 20 Tm (A) Tj 0 -5 Td (A) Tj ET");
    let runs = texts(&events);
    assert_matrix(runs[0].glyphs[0].matrix, [20.0, 0.0, 0.0, 20.0, 10.0, 20.0]);
    // Td moves in the scaled text space: -5 * 2 in y.
    assert_matrix(runs[1].glyphs[0].matrix, [20.0, 0.0, 0.0, 20.0, 10.0, 10.0]);
}

#[test]
fn the_quote_operator_starts_a_new_line_first() {
    let (_, events) = run_text("BT /F1 10 Tf 14 TL (A) ' ET");
    assert_matrix(
        texts(&events)[0].glyphs[0].matrix,
        [10.0, 0.0, 0.0, 10.0, 0.0, -14.0],
    );
}

#[test]
fn the_double_quote_operator_sets_the_spacing_then_shows() {
    let (_, events) = run_text("BT /F1 10 Tf 14 TL 5 2 (A) \" ET");
    let Event::Text { state, run, .. } = events
        .iter()
        .find(|e| matches!(e, Event::Text { .. }))
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(state.text.word_spacing, 5.0);
    assert_eq!(state.text.char_spacing, 2.0);
    assert_matrix(run.glyphs[0].matrix, [10.0, 0.0, 0.0, 10.0, 0.0, -14.0]);
    // A is not a space: 6 + Tc.
    assert_eq!(run.glyphs[0].advance, (8.0, 0.0));
}

#[test]
fn bt_starts_the_matrices_again() {
    let (_, events) = run_text("BT /F1 10 Tf 10 20 Td ET BT (A) Tj ET");
    assert_matrix(
        texts(&events)[0].glyphs[0].matrix,
        [10.0, 0.0, 0.0, 10.0, 0.0, 0.0],
    );
}

#[test]
fn the_text_state_outlives_the_text_object() {
    let (_, events) = run_text("/F1 10 Tf 3 Tc BT ET BT (A) Tj ET");
    let Event::Text { state, .. } = events
        .iter()
        .find(|e| matches!(e, Event::Text { .. }))
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(state.text.char_spacing, 3.0);
}

#[test]
fn the_render_mode_is_in_the_state() {
    let (_, events) = run_text("BT /F1 10 Tf 1 Tr (A) Tj ET");
    let Event::Text { state, .. } = events
        .iter()
        .find(|e| matches!(e, Event::Text { .. }))
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(state.text.render_mode, RenderMode::Stroke);
}

#[test]
fn a_render_mode_outside_0_to_7_is_invalid() {
    let (document, events) = run_text("BT /F1 10 Tf 8 Tr (A) Tj ET");
    let Event::Text { state, .. } = events
        .iter()
        .find(|e| matches!(e, Event::Text { .. }))
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(state.text.render_mode, RenderMode::Fill);
    assert!(has(
        &document,
        |k| matches!(k, DiagnosticKind::OperandsInvalid { operator } if operator.as_bytes() == b"Tr")
    ));
}

#[test]
fn the_font_is_read_once_for_each_object() {
    let (_, events) = run_text("BT /F1 10 Tf (A) Tj /F1 12 Tf (B) Tj ET");
    let runs = texts(&events);
    assert!(Arc::ptr_eq(&runs[0].font, &runs[1].font));
    assert_eq!(runs[1].font_size, 12.0);
}

#[test]
fn a_font_not_in_the_resources_is_reported_and_text_is_skipped() {
    let (document, events) = run_text("BT /Nope 10 Tf (A) Tj ET");
    assert!(texts(&events).is_empty());
    assert!(has(
        &document,
        |k| matches!(k, DiagnosticKind::ResourceMissing { category, .. } if category.as_bytes() == b"Font")
    ));
    assert!(has(
        &document,
        |k| matches!(k, DiagnosticKind::OperatorMisplaced { operator } if operator.as_bytes() == b"Tj")
    ));
}

#[test]
fn text_before_any_font_is_skipped_and_reported() {
    let (document, events) = run_text("BT (A) Tj ET");
    assert!(texts(&events).is_empty());
    assert!(has(&document, |k| matches!(
        k,
        DiagnosticKind::OperatorMisplaced { .. }
    )));
}

#[test]
fn a_font_that_cannot_be_read_is_reported_and_text_is_skipped() {
    let resources = "/Font << /F9 62 0 R >>";
    let extra: [(u32, &[u8]); 1] = [(62, b"<< /Type /Font /Subtype /Nonsense >>")];
    let (document, events) = run("BT /F9 10 Tf (A) Tj ET", resources, &extra);
    assert!(texts(&events).is_empty());
    assert!(has(
        &document,
        |k| matches!(k, DiagnosticKind::FontUnreadable { name } if name.as_bytes() == b"F9")
    ));
}

#[test]
fn showing_outside_a_text_object_is_reported_and_done() {
    let (document, events) = run_text("/F1 10 Tf (A) Tj");
    assert_eq!(texts(&events).len(), 1);
    assert!(has(
        &document,
        |k| matches!(k, DiagnosticKind::OperatorMisplaced { operator } if operator.as_bytes() == b"Tj")
    ));
}

#[test]
fn bt_inside_a_text_object_and_et_outside_one_are_misplaced() {
    let (document, _) = run_text("BT BT ET ET");
    let misplaced = document
        .diagnostics()
        .iter()
        .filter(|d| matches!(d.kind, DiagnosticKind::OperatorMisplaced { .. }))
        .count();
    assert_eq!(misplaced, 2);
}

#[test]
fn a_string_operand_that_is_not_a_string_is_invalid() {
    let (document, events) = run_text("BT /F1 10 Tf 5 Tj ET");
    assert!(texts(&events).is_empty());
    assert!(has(&document, |k| matches!(
        k,
        DiagnosticKind::OperandsInvalid { .. }
    )));
}

#[test]
fn a_tj_item_that_is_neither_string_nor_number_is_invalid_and_skipped() {
    let (document, events) = run_text("BT /F1 10 Tf [(A) /Name (B)] TJ ET");
    assert_eq!(texts(&events)[0].glyphs.len(), 2);
    assert!(has(&document, |k| matches!(
        k,
        DiagnosticKind::OperandsInvalid { .. }
    )));
}

#[test]
fn begin_and_end_text_are_events() {
    let (_, events) = run_text("BT /F1 10 Tf (A) Tj ET");
    assert!(matches!(events[0], Event::BeginText));
    assert!(matches!(events.last().unwrap(), Event::EndText));
}

#[test]
fn vertical_writing_moves_down_and_puts_the_origin_at_the_position_vector() {
    // Identity-V, DW 1000 and the default DW2 [880 -1000] (Table 117): the
    // position vector is (w0 / 2, 880) and the displacement is -1000. With
    // Tfs 10 the glyph is drawn at (-0.5, -0.88) em from its origin and the
    // next one is 10 units lower.
    let resources = "/Font << /FV 70 0 R >>";
    let extra: [(u32, &[u8]); 3] = [
        (
            70,
            b"<< /Type /Font /Subtype /Type0 /BaseFont /Foo /Encoding /Identity-V /DescendantFonts [71 0 R] >>",
        ),
        (
            71,
            b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Foo \
              /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> \
              /FontDescriptor 72 0 R /DW 1000 >>",
        ),
        (72, b"<< /Type /FontDescriptor /FontName /Foo /Flags 4 >>"),
    ];
    let (_, events) = run(
        "BT /FV 10 Tf 100 700 Td <00010002> Tj ET",
        resources,
        &extra,
    );
    let glyphs = &texts(&events)[0].glyphs;
    assert_eq!(glyphs.len(), 2);
    assert_matrix(
        glyphs[0].matrix,
        [10.0, 0.0, 0.0, 10.0, 100.0 - 5.0, 700.0 - 8.8],
    );
    assert_eq!(glyphs[0].advance, (0.0, -10.0));
    assert_matrix(
        glyphs[1].matrix,
        [10.0, 0.0, 0.0, 10.0, 95.0, 700.0 - 10.0 - 8.8],
    );
}
