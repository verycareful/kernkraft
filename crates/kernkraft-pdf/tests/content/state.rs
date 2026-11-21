// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The graphics state: ISO 32000-1, 8.4 (Tables 52 and 57), 8.6.8 (colour
//! operators) and 11.6 (Table 58).

use kernkraft_pdf::content::{ColorSpace, Dash, LineCap, LineJoin, SoftMask};
use kernkraft_pdf::DiagnosticKind;

use super::{assert_matrix, has, name, paths, run, Event};

/// The state at the last path painted.
fn last_state(events: &[Event]) -> kernkraft_pdf::content::GraphicsState {
    match paths(events).last().expect("a path was painted") {
        Event::Path { state, .. } => state.clone(),
        _ => unreachable!(),
    }
}

const DOT: &str = " 0 0 m 1 1 l S";

#[test]
fn the_state_starts_as_table_52_says() {
    let (_, events) = run(DOT, "", &[]);
    let state = last_state(&events);
    assert_eq!(state.line_width, 1.0);
    assert_eq!(state.line_cap, LineCap::Butt);
    assert_eq!(state.line_join, LineJoin::Miter);
    assert_eq!(state.miter_limit, 10.0);
    assert_eq!(state.dash, Dash::default());
    assert_eq!(state.fill_alpha, 1.0);
    assert_eq!(state.stroke_alpha, 1.0);
    assert_eq!(state.blend_mode, name("Normal"));
    assert_eq!(state.soft_mask, SoftMask::None);
    assert_eq!(state.fill.space, ColorSpace::DeviceGray);
    assert_eq!(state.fill.components, [0.0]);
    assert_eq!(state.stroke.components, [0.0]);
    assert_matrix(state.ctm, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
}

#[test]
fn line_style_operators_set_the_state() {
    let (document, events) = run(&format!("7 w 1 J 2 j 4 M [3 2] 1 d{DOT}"), "", &[]);
    let state = last_state(&events);
    assert_eq!(state.line_width, 7.0);
    assert_eq!(state.line_cap, LineCap::Round);
    assert_eq!(state.line_join, LineJoin::Bevel);
    assert_eq!(state.miter_limit, 4.0);
    assert_eq!(
        state.dash,
        Dash {
            array: vec![3.0, 2.0],
            phase: 1.0
        }
    );
    assert!(document.diagnostics().is_empty());
}

#[test]
fn a_cap_outside_0_to_2_is_invalid_and_changes_nothing() {
    let (document, events) = run(&format!("3 J{DOT}"), "", &[]);
    assert_eq!(last_state(&events).line_cap, LineCap::Butt);
    assert!(has(
        &document,
        |k| matches!(k, DiagnosticKind::OperandsInvalid { operator } if operator.as_bytes() == b"J")
    ));
}

#[test]
fn a_negative_dash_length_is_invalid() {
    let (document, events) = run(&format!("[3 -2] 0 d{DOT}"), "", &[]);
    assert_eq!(last_state(&events).dash, Dash::default());
    assert!(has(&document, |k| matches!(
        k,
        DiagnosticKind::OperandsInvalid { .. }
    )));
}

#[test]
fn the_wrong_number_of_operands_is_invalid_and_changes_nothing() {
    let (document, events) = run(&format!("2 3 w{DOT}"), "", &[]);
    assert_eq!(last_state(&events).line_width, 1.0);
    assert!(has(
        &document,
        |k| matches!(k, DiagnosticKind::OperandsInvalid { operator } if operator.as_bytes() == b"w")
    ));
}

#[test]
fn cm_puts_the_new_matrix_first() {
    // The second cm is applied before the first: (1 0 0 1 5 5) x (2 0 0 2 10 10)
    // moves by 5, scales by 2, moves by 10: e = 5 * 2 + 10.
    let (_, events) = run(&format!("2 0 0 2 10 10 cm 1 0 0 1 5 5 cm{DOT}"), "", &[]);
    assert_matrix(last_state(&events).ctm, [2.0, 0.0, 0.0, 2.0, 20.0, 20.0]);
}

#[test]
fn q_and_big_q_save_and_restore_the_state() {
    let (document, events) = run(&format!("5 w q 9 w 2 0 0 2 0 0 cm{DOT} Q{DOT}"), "", &[]);
    let paths = paths(&events);
    let Event::Path { state: inside, .. } = paths[0] else {
        panic!()
    };
    let Event::Path { state: after, .. } = paths[1] else {
        panic!()
    };
    assert_eq!(inside.line_width, 9.0);
    assert_matrix(inside.ctm, [2.0, 0.0, 0.0, 2.0, 0.0, 0.0]);
    assert_eq!(after.line_width, 5.0);
    assert_matrix(after.ctm, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    assert!(document.diagnostics().is_empty());
    assert!(matches!(events[0], Event::Save));
}

#[test]
fn a_big_q_with_no_q_is_ignored_and_reported() {
    let (document, events) = run(&format!("3 w Q{DOT}"), "", &[]);
    assert_eq!(last_state(&events).line_width, 3.0);
    assert!(has(&document, |k| *k == DiagnosticKind::GraphicsStateUnderflow));
    assert!(!events.iter().any(|e| matches!(e, Event::Restore)));
}

#[test]
fn a_q_left_open_is_closed_at_the_end_of_the_stream() {
    let (document, events) = run("q q", "", &[]);
    assert!(has(&document, |k| *k == DiagnosticKind::GraphicsStateUnclosed));
    let restores = events
        .iter()
        .filter(|e| matches!(e, Event::Restore))
        .count();
    assert_eq!(restores, 2);
}

#[test]
fn device_colour_operators_set_the_space_and_components() {
    let (_, events) = run(&format!("0.1 0.2 0.3 rg 1 0 0 0 K{DOT}"), "", &[]);
    let state = last_state(&events);
    assert_eq!(state.fill.space, ColorSpace::DeviceRGB);
    assert_eq!(state.fill.components, [0.1, 0.2, 0.3]);
    assert_eq!(state.stroke.space, ColorSpace::DeviceCMYK);
    assert_eq!(state.stroke.components, [1.0, 0.0, 0.0, 0.0]);
}

#[test]
fn a_device_colour_with_too_few_components_is_invalid() {
    let (document, events) = run(&format!("0.1 0.2 rg{DOT}"), "", &[]);
    assert_eq!(last_state(&events).fill.space, ColorSpace::DeviceGray);
    assert!(has(&document, |k| matches!(
        k,
        DiagnosticKind::OperandsInvalid { .. }
    )));
}

#[test]
fn cs_starts_a_space_at_its_initial_colour() {
    // 8.6.6.4: a Separation space starts at its full tint, 1.0; 8.6.5.5:
    // ICCBased starts at 0 in each of its N components; 8.6.6.5: DeviceN at
    // 1.0 in each colorant.
    let resources = "/ColorSpace << /Sep [/Separation /Spot /DeviceRGB 40 0 R] \
                     /Icc [/ICCBased 41 0 R] /DN [/DeviceN [/A /B] /DeviceRGB 40 0 R] >>";
    let extra: [(u32, &[u8]); 2] = [
        (40, b"<< /FunctionType 2 /Domain [0 1] /N 1 >>"),
        (41, b"<< /N 3 /Length 0 >>\nstream\n\nendstream"),
    ];
    let (_, events) = run(&format!("/Sep CS /Icc cs{DOT}"), resources, &extra);
    let state = last_state(&events);
    assert_eq!(state.stroke.components, [1.0]);
    assert!(
        matches!(state.stroke.space, ColorSpace::Resource { ref name, .. } if name.as_bytes() == b"Sep")
    );
    assert_eq!(state.fill.components, [0.0, 0.0, 0.0]);
    let (_, events) = run(&format!("/DN cs{DOT}"), resources, &extra);
    assert_eq!(last_state(&events).fill.components, [1.0, 1.0]);
}

#[test]
fn cs_names_the_device_spaces_without_a_resource() {
    let (document, events) = run(&format!("/DeviceCMYK cs{DOT}"), "", &[]);
    let state = last_state(&events);
    assert_eq!(state.fill.space, ColorSpace::DeviceCMYK);
    assert_eq!(state.fill.components, [0.0, 0.0, 0.0, 1.0]);
    assert!(document.diagnostics().is_empty());
}

#[test]
fn an_unknown_colour_space_is_a_missing_resource() {
    let (document, _) = run("/Nope cs", "", &[]);
    assert!(has(
        &document,
        |k| matches!(k, DiagnosticKind::ResourceMissing { category, name } if category.as_bytes() == b"ColorSpace" && name.as_bytes() == b"Nope")
    ));
}

#[test]
fn scn_in_a_pattern_space_names_the_pattern() {
    let resources = "/ColorSpace << /Cs [/Pattern /DeviceRGB] >> /Pattern << /P1 42 0 R >>";
    let extra: [(u32, &[u8]); 1] = [(42, b"<< /PatternType 1 >>")];
    let (document, events) = run(&format!("/Cs cs 1 0 0 /P1 scn{DOT}"), resources, &extra);
    let state = last_state(&events);
    assert_eq!(state.fill.components, [1.0, 0.0, 0.0]);
    let (pattern_name, _) = state.fill.pattern.expect("a pattern");
    assert_eq!(pattern_name, name("P1"));
    assert!(document.diagnostics().is_empty());
}

#[test]
fn a_pattern_name_in_a_non_pattern_space_is_misplaced() {
    let resources = "/Pattern << /P1 42 0 R >>";
    let extra: [(u32, &[u8]); 1] = [(42, b"<< /PatternType 1 >>")];
    let (document, events) = run(&format!("/P1 scn{DOT}"), resources, &extra);
    assert!(last_state(&events).fill.pattern.is_none());
    assert!(has(&document, |k| matches!(
        k,
        DiagnosticKind::OperatorMisplaced { .. }
    )));
}

#[test]
fn gs_sets_the_state_from_an_extended_graphics_state() {
    let resources =
        "/ExtGState << /G1 << /LW 4 /LC 2 /LJ 1 /ML 3 /CA 0.5 /ca 0.25 /BM [/Multiply /Normal] \
                     /AIS true /TK false /OP true /op false /OPM 1 /SMask /None /D [[2 1] 0] >> >>";
    let (document, events) = run(&format!("/G1 gs{DOT}"), resources, &[]);
    let state = last_state(&events);
    assert_eq!(state.line_width, 4.0);
    assert_eq!(state.line_cap, LineCap::Square);
    assert_eq!(state.line_join, LineJoin::Round);
    assert_eq!(state.miter_limit, 3.0);
    assert_eq!(state.stroke_alpha, 0.5);
    assert_eq!(state.fill_alpha, 0.25);
    assert_eq!(state.blend_mode, name("Multiply"));
    assert!(state.alpha_is_shape);
    assert!(!state.text_knockout);
    assert!(state.stroke_overprint);
    assert!(!state.fill_overprint);
    assert_eq!(state.overprint_mode, 1);
    assert_eq!(state.dash.array, [2.0, 1.0]);
    assert!(document.diagnostics().is_empty());
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::ExtGState { name: n, .. } if *n == name("G1"))));
}

#[test]
fn an_alpha_is_held_in_0_to_1() {
    let resources = "/ExtGState << /G1 << /CA 3 /ca -1 >> >>";
    let (_, events) = run(&format!("/G1 gs{DOT}"), resources, &[]);
    let state = last_state(&events);
    assert_eq!(state.stroke_alpha, 1.0);
    assert_eq!(state.fill_alpha, 0.0);
}

#[test]
fn a_soft_mask_dictionary_is_kept() {
    let resources = "/ExtGState << /G1 << /SMask << /Type /Mask /S /Alpha >> >> >>";
    let (_, events) = run(&format!("/G1 gs{DOT}"), resources, &[]);
    assert!(matches!(
        last_state(&events).soft_mask,
        SoftMask::Dictionary(_)
    ));
}

#[test]
fn an_unknown_extended_graphics_state_is_a_missing_resource() {
    let (document, _) = run("/G9 gs", "", &[]);
    assert!(has(
        &document,
        |k| matches!(k, DiagnosticKind::ResourceMissing { category, .. } if category.as_bytes() == b"ExtGState")
    ));
}

#[test]
fn a_bad_entry_of_an_extended_graphics_state_is_reported_and_the_rest_applies() {
    let resources = "/ExtGState << /G1 << /LW (x) /ML 6 >> >>";
    let (document, events) = run(&format!("/G1 gs{DOT}"), resources, &[]);
    let state = last_state(&events);
    assert_eq!(state.line_width, 1.0);
    assert_eq!(state.miter_limit, 6.0);
    assert!(has(&document, |k| matches!(
        k,
        DiagnosticKind::OperandsInvalid { .. }
    )));
}

#[test]
fn an_unknown_operator_is_reported_and_skipped() {
    let (document, events) = run(&format!("1 2 xyz{DOT}"), "", &[]);
    assert_eq!(paths(&events).len(), 1);
    assert!(has(
        &document,
        |k| matches!(k, DiagnosticKind::OperatorUnknown { operator } if operator.as_bytes() == b"xyz")
    ));
}

#[test]
fn an_unknown_operator_inside_bx_ex_is_ignored_silently() {
    let (document, events) = run(&format!("BX 1 2 xyz EX{DOT}"), "", &[]);
    assert_eq!(paths(&events).len(), 1);
    assert!(document.diagnostics().is_empty());
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::Compatibility(true))));
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::Compatibility(false))));
}

#[test]
fn ex_without_bx_is_misplaced() {
    let (document, _) = run("EX", "", &[]);
    assert!(has(&document, |k| matches!(
        k,
        DiagnosticKind::OperatorMisplaced { .. }
    )));
}

#[test]
fn the_open_graphics_state_depth_is_limited() {
    use kernkraft_pdf::syntax::limits::MAX_GRAPHICS_STATE_DEPTH;
    let content = "q ".repeat(MAX_GRAPHICS_STATE_DEPTH + 1);
    let document = super::content_fixture(content.as_bytes(), "", &[]).open();
    let error = super::run_document(&document).unwrap_err();
    assert!(matches!(
        error,
        kernkraft_pdf::PdfError::LimitExceeded {
            limit: kernkraft_pdf::Limit::GraphicsStateDepth,
            ..
        }
    ));
}
