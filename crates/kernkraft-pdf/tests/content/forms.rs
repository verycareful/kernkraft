// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! `Do`: ISO 32000-1, 8.8 and 8.10 (Table 95).

use kernkraft_fs::NoProgress;
use kernkraft_pdf::content::{
    FillRule, ImageSource, InterpretOptions, Interpreter, Matrix, StreamId,
};
use kernkraft_pdf::syntax::limits::MAX_FORM_DEPTH;
use kernkraft_pdf::{DiagnosticKind, Limit, PdfError};

use super::{
    assert_matrix, content_fixture, has, id, name, paths, run, run_document, texts, Event,
    Recorder, FONTS, FONT_OBJECTS,
};

fn form(entries: &str, content: &str) -> Vec<u8> {
    super::stream(
        &format!("/Type /XObject /Subtype /Form {entries}"),
        content.as_bytes(),
    )
}

const DOT: &str = "0 0 m 1 1 l S";

#[test]
fn a_form_is_run_inside_its_own_saved_state() {
    let form = form("/BBox [0 0 100 50] /Matrix [2 0 0 2 10 10]", DOT);
    let (document, events) = run(
        "q 1 0 0 1 5 5 cm /Fm1 Do Q",
        "/XObject << /Fm1 40 0 R >>",
        &[(40, &form)],
    );
    let kinds: Vec<&str> = events
        .iter()
        .map(|event| match event {
            Event::Save => "save",
            Event::Restore => "restore",
            Event::BeginForm(..) => "begin",
            Event::EndForm(_) => "end",
            Event::Clip { .. } => "clip",
            Event::Path { .. } => "path",
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        kinds,
        ["save", "save", "begin", "clip", "path", "end", "restore", "restore"]
    );
    assert!(document.diagnostics().is_empty());
}

#[test]
fn the_form_matrix_goes_in_front_of_the_ctm() {
    // The form matrix applies first: [2 0 0 2 10 10] x [1 0 0 1 5 5].
    let form = form("/BBox [0 0 100 50] /Matrix [2 0 0 2 10 10]", DOT);
    let (_, events) = run(
        "1 0 0 1 5 5 cm /Fm1 Do",
        "/XObject << /Fm1 40 0 R >>",
        &[(40, &form)],
    );
    let Event::BeginForm(info, state) = events
        .iter()
        .find(|e| matches!(e, Event::BeginForm(..)))
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(info.id, id(40));
    assert_eq!(info.name, Some(name("Fm1")));
    assert_matrix(state.ctm, [2.0, 0.0, 0.0, 2.0, 15.0, 15.0]);
    let Event::Path { state, .. } = paths(&events)[0] else {
        panic!()
    };
    assert_matrix(state.ctm, [2.0, 0.0, 0.0, 2.0, 15.0, 15.0]);
}

#[test]
fn the_state_after_a_form_is_the_state_before() {
    let form = form("/BBox [0 0 1 1] /Matrix [3 0 0 3 0 0]", "9 w");
    let (_, events) = run(
        "2 w /Fm1 Do 0 0 m 1 1 l S",
        "/XObject << /Fm1 40 0 R >>",
        &[(40, &form)],
    );
    let Event::Path { state, .. } = paths(&events)[0] else {
        panic!()
    };
    assert_eq!(state.line_width, 2.0);
    assert_matrix(state.ctm, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
}

#[test]
fn the_bbox_is_a_clip_in_form_space() {
    let form = form("/BBox [100 50 0 0]", DOT);
    let (_, events) = run("/Fm1 Do", "/XObject << /Fm1 40 0 R >>", &[(40, &form)]);
    let Event::Clip { path, rule } = events
        .iter()
        .find(|e| matches!(e, Event::Clip { .. }))
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(*rule, FillRule::NonZeroWinding);
    use kernkraft_pdf::content::PathSegment::{Close, LineTo, MoveTo};
    // The corners are put in order: 0 0 to 100 50.
    assert_eq!(
        path.segments,
        [
            MoveTo(0.0, 0.0),
            LineTo(100.0, 0.0),
            LineTo(100.0, 50.0),
            LineTo(0.0, 50.0),
            Close
        ]
    );
}

#[test]
fn a_form_uses_its_own_resources() {
    let form = form(
        "/BBox [0 0 100 50] /Resources << /Font << /FF 60 0 R >> >>",
        "BT /FF 10 Tf (A) Tj ET",
    );
    let mut extra = FONT_OBJECTS.to_vec();
    extra.push((40, &form));
    let (document, events) = run("/Fm1 Do", "/XObject << /Fm1 40 0 R >>", &extra);
    assert_eq!(texts(&events).len(), 1);
    assert!(document.diagnostics().is_empty());
}

#[test]
fn a_form_with_no_resources_uses_the_enclosing_ones() {
    let form = form("/BBox [0 0 100 50]", "BT /F1 10 Tf (A) Tj ET");
    let mut extra = FONT_OBJECTS.to_vec();
    extra.push((40, &form));
    let resources = format!("{FONTS} /XObject << /Fm1 40 0 R >>");
    let (document, events) = run("/Fm1 Do", &resources, &extra);
    assert_eq!(texts(&events).len(), 1);
    assert!(document.diagnostics().is_empty());
}

#[test]
fn spans_inside_a_form_are_in_the_forms_stream() {
    let form = form("/BBox [0 0 100 50]", &format!("q {DOT}"));
    let (_, events) = run("/Fm1 Do", "/XObject << /Fm1 40 0 R >>", &[(40, &form)]);
    let Event::Path { span, .. } = paths(&events)[0] else {
        panic!()
    };
    assert_eq!(span.stream, StreamId::Object(id(40)));
    // q 0, m 1, l 2, S 3.
    assert_eq!(span.index, 3);
}

#[test]
fn a_transparency_group_is_in_the_form_info() {
    let form = form("/BBox [0 0 1 1] /Group << /S /Transparency /I true >>", DOT);
    let (_, events) = run("/Fm1 Do", "/XObject << /Fm1 40 0 R >>", &[(40, &form)]);
    let Event::BeginForm(info, _) = events
        .iter()
        .find(|e| matches!(e, Event::BeginForm(..)))
        .unwrap()
    else {
        panic!()
    };
    assert!(info.group.as_ref().unwrap().get(b"S").is_some());
}

#[test]
fn a_form_that_paints_itself_is_skipped_and_reported() {
    let form = form(
        "/BBox [0 0 1 1] /Resources << /XObject << /Fm1 40 0 R >> >>",
        &format!("/Fm1 Do {DOT}"),
    );
    let (document, events) = run("/Fm1 Do", "/XObject << /Fm1 40 0 R >>", &[(40, &form)]);
    assert_eq!(paths(&events).len(), 1);
    let recursion: Vec<_> = document
        .diagnostics()
        .into_iter()
        .filter(|d| d.kind == DiagnosticKind::FormRecursion)
        .collect();
    assert_eq!(recursion.len(), 1);
    assert_eq!(recursion[0].object, Some(id(40)));
}

#[test]
fn two_forms_that_paint_each_other_stop_at_the_second_visit() {
    let a = form(
        "/BBox [0 0 1 1] /Resources << /XObject << /B 41 0 R >> >>",
        "/B Do",
    );
    let b = form(
        "/BBox [0 0 1 1] /Resources << /XObject << /A 40 0 R >> >>",
        "/A Do",
    );
    let (document, events) = run("/A Do", "/XObject << /A 40 0 R >>", &[(40, &a), (41, &b)]);
    let begins = events
        .iter()
        .filter(|e| matches!(e, Event::BeginForm(..)))
        .count();
    let ends = events
        .iter()
        .filter(|e| matches!(e, Event::EndForm(_)))
        .count();
    assert_eq!((begins, ends), (2, 2));
    assert!(has(&document, |k| *k == DiagnosticKind::FormRecursion));
}

#[test]
fn a_chain_of_forms_past_the_depth_limit_is_an_error() {
    let mut objects: Vec<(u32, Vec<u8>)> = Vec::new();
    let count = MAX_FORM_DEPTH as u32 + 2;
    for index in 0..count {
        let number = 100 + index;
        let next = number + 1;
        let content = if index + 1 < count { "/N Do" } else { "" };
        objects.push((
            number,
            form(
                &format!("/BBox [0 0 1 1] /Resources << /XObject << /N {next} 0 R >> >>"),
                content,
            ),
        ));
    }
    let extra: Vec<(u32, &[u8])> = objects.iter().map(|(n, b)| (*n, b.as_slice())).collect();
    let document = content_fixture(b"/N Do", "/XObject << /N 100 0 R >>", &extra).open();
    assert!(matches!(
        run_document(&document).unwrap_err(),
        PdfError::LimitExceeded {
            limit: Limit::FormNesting,
            ..
        }
    ));
}

#[test]
fn a_stray_big_q_in_a_form_cannot_pop_the_callers_state() {
    let form = form("/BBox [0 0 1 1]", "Q Q");
    let (document, events) = run(
        "5 w q /Fm1 Do 0 0 m 1 1 l S Q",
        "/XObject << /Fm1 40 0 R >>",
        &[(40, &form)],
    );
    let Event::Path { state, .. } = paths(&events)[0] else {
        panic!()
    };
    assert_eq!(state.line_width, 5.0);
    let underflows = document
        .diagnostics()
        .iter()
        .filter(|d| d.kind == DiagnosticKind::GraphicsStateUnderflow)
        .count();
    assert_eq!(underflows, 2);
    // Two saves (the q and the form), two restores (the form's end, then Q).
    let restores = events
        .iter()
        .filter(|e| matches!(e, Event::Restore))
        .count();
    assert_eq!(restores, 2);
}

#[test]
fn a_q_left_open_in_a_form_does_not_leak_out() {
    let form = form("/BBox [0 0 1 1]", "q 9 w");
    let (document, events) = run(
        "3 w /Fm1 Do 0 0 m 1 1 l S",
        "/XObject << /Fm1 40 0 R >>",
        &[(40, &form)],
    );
    let Event::Path { state, .. } = paths(&events)[0] else {
        panic!()
    };
    assert_eq!(state.line_width, 3.0);
    assert!(has(&document, |k| *k == DiagnosticKind::GraphicsStateUnclosed));
}

#[test]
fn an_image_xobject_is_an_image_event() {
    let image = super::stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray", b"\x00");
    let (_, events) = run(
        "2 0 0 2 0 0 cm /Im1 Do",
        "/XObject << /Im1 41 0 R >>",
        &[(41, &image)],
    );
    let Event::Image {
        image,
        state,
        visible,
    } = events
        .iter()
        .find(|e| matches!(e, Event::Image { .. }))
        .unwrap()
    else {
        panic!()
    };
    assert!(
        matches!(image, ImageSource::XObject { name: n, id: i } if *n == name("Im1") && *i == id(41))
    );
    assert_eq!(state.ctm.a, 2.0);
    assert!(*visible);
}

#[test]
fn an_inline_image_is_an_image_event() {
    let (_, events) = run("BI /W 1 /H 1 /BPC 8 /CS /G ID a EI", "", &[]);
    let Event::Image { image, .. } = &events[0] else {
        panic!()
    };
    let ImageSource::Inline(inline) = image else {
        panic!()
    };
    assert!(inline.dictionary.get(b"Width").is_some());
}

#[test]
fn a_missing_xobject_is_reported() {
    let (document, events) = run("/Nope Do", "", &[]);
    assert!(events.is_empty());
    assert!(has(
        &document,
        |k| matches!(k, DiagnosticKind::ResourceMissing { category, .. } if category.as_bytes() == b"XObject")
    ));
}

#[test]
fn a_postscript_xobject_is_unsupported() {
    let ps = super::stream("/Type /XObject /Subtype /PS", b"stuff");
    let (document, events) = run("/P Do", "/XObject << /P 40 0 R >>", &[(40, &ps)]);
    assert!(events.is_empty());
    assert!(has(
        &document,
        |k| matches!(k, DiagnosticKind::XObjectSubtypeUnsupported { subtype } if subtype.as_bytes() == b"PS")
    ));
}

#[test]
fn run_form_runs_a_form_in_a_given_matrix() {
    let form = form("/BBox [0 0 10 10] /Matrix [1 0 0 1 3 4]", DOT);
    let document = content_fixture(b"", "", &[(40, &form)]).open();
    let mut interpreter = Interpreter::new(&document, InterpretOptions::default()).unwrap();
    let mut recorder = Recorder::default();
    interpreter
        .run_form(
            id(40),
            Matrix::scaling(2.0, 2.0),
            &mut recorder,
            &NoProgress,
        )
        .unwrap();
    let Event::Path { state, .. } = paths(&recorder.events)[0] else {
        panic!()
    };
    assert_matrix(state.ctm, [2.0, 0.0, 0.0, 2.0, 6.0, 8.0]);
}

#[test]
fn the_base_matrix_option_starts_the_ctm() {
    let document = content_fixture(b"0 0 m 1 1 l S", "", &[]).open();
    let options = InterpretOptions {
        base_matrix: Matrix::new(1.0, 0.0, 0.0, -1.0, 0.0, 792.0),
        ..InterpretOptions::default()
    };
    let events = super::run_document_with(&document, options).unwrap();
    let Event::Path { state, .. } = paths(&events)[0] else {
        panic!()
    };
    assert_matrix(state.ctm, [1.0, 0.0, 0.0, -1.0, 0.0, 792.0]);
}

#[test]
fn forms_that_multiply_are_stopped_by_the_operation_limit() {
    // Each of 40 forms paints the next twice: 2^40 operations in all.
    let count = 40u32;
    let mut objects: Vec<(u32, Vec<u8>)> = Vec::new();
    for index in 0..count {
        let number = 100 + index;
        let content = if index + 1 < count {
            "/N Do /N Do"
        } else {
            "0 0 m 1 1 l S"
        };
        objects.push((
            number,
            form(
                &format!(
                    "/BBox [0 0 1 1] /Resources << /XObject << /N {} 0 R >> >>",
                    number + 1
                ),
                content,
            ),
        ));
    }
    let extra: Vec<(u32, &[u8])> = objects.iter().map(|(n, b)| (*n, b.as_slice())).collect();
    let document = content_fixture(b"/N Do", "/XObject << /N 100 0 R >>", &extra).open();
    let options = InterpretOptions {
        max_operations: 10_000,
        ..InterpretOptions::default()
    };
    assert!(matches!(
        super::run_document_with(&document, options).unwrap_err(),
        PdfError::LimitExceeded {
            limit: Limit::Operations { max: 10_000 },
            ..
        }
    ));
}

#[test]
fn the_operation_count_starts_again_for_each_run() {
    let document = content_fixture(b"q Q q Q", "", &[]).open();
    let options = InterpretOptions {
        max_operations: 4,
        ..InterpretOptions::default()
    };
    let page = document.page(1).unwrap();
    let mut interpreter = Interpreter::new(&document, options).unwrap();
    for _ in 0..3 {
        interpreter
            .run_page(&page, &mut Recorder::default(), &NoProgress)
            .unwrap();
    }
}
