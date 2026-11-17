// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Paths and clipping: ISO 32000-1, 8.5 (Tables 59 to 61).

use kernkraft_pdf::content::{FillRule, Path, PathPaint, PathSegment};
use kernkraft_pdf::DiagnosticKind;

use super::{has, paths, run, Event};

fn only_path(events: &[Event]) -> (&Path, PathPaint) {
    let paths = paths(events);
    assert_eq!(paths.len(), 1, "{events:?}");
    match paths[0] {
        Event::Path { path, paint, .. } => (path, *paint),
        _ => unreachable!(),
    }
}

use PathSegment::{Close, CurveTo, LineTo, MoveTo};

#[test]
fn a_stroked_line() {
    let (_, events) = run("10 20 m 30 40 l S", "", &[]);
    let (path, paint) = only_path(&events);
    assert_eq!(path.segments, [MoveTo(10.0, 20.0), LineTo(30.0, 40.0)]);
    assert_eq!(
        paint,
        PathPaint {
            fill: None,
            stroke: true
        }
    );
}

#[test]
fn a_rectangle_is_four_lines_and_a_close() {
    // 8.5.2.1: x y w h re is x y m, x+w y l, x+w y+h l, x y+h l, h.
    let (_, events) = run("1 2 10 20 re f", "", &[]);
    let (path, paint) = only_path(&events);
    assert_eq!(
        path.segments,
        [
            MoveTo(1.0, 2.0),
            LineTo(11.0, 2.0),
            LineTo(11.0, 22.0),
            LineTo(1.0, 22.0),
            Close
        ]
    );
    assert_eq!(paint.fill, Some(FillRule::NonZeroWinding));
    assert!(!paint.stroke);
}

#[test]
fn each_painting_operator_does_what_table_60_says() {
    let cases: [(&str, Option<FillRule>, bool); 8] = [
        ("S", None, true),
        ("f", Some(FillRule::NonZeroWinding), false),
        ("F", Some(FillRule::NonZeroWinding), false),
        ("f*", Some(FillRule::EvenOdd), false),
        ("B", Some(FillRule::NonZeroWinding), true),
        ("B*", Some(FillRule::EvenOdd), true),
        ("b", Some(FillRule::NonZeroWinding), true),
        ("b*", Some(FillRule::EvenOdd), true),
    ];
    for (operator, fill, stroke) in cases {
        let (_, events) = run(&format!("0 0 m 5 5 l 0 5 l {operator}"), "", &[]);
        let (_, paint) = only_path(&events);
        assert_eq!(paint, PathPaint { fill, stroke }, "{operator}");
    }
}

#[test]
fn s_b_and_b_star_close_the_subpath_first() {
    for operator in ["s", "b", "b*"] {
        let (_, events) = run(&format!("0 0 m 5 5 l 0 5 l {operator}"), "", &[]);
        let (path, _) = only_path(&events);
        assert_eq!(path.segments.last(), Some(&Close), "{operator}");
    }
    let (_, events) = run("0 0 m 5 5 l 0 5 l S", "", &[]);
    let (path, _) = only_path(&events);
    assert_ne!(path.segments.last(), Some(&Close));
}

#[test]
fn curves_with_v_and_y() {
    // v: the first control point is the current point. y: the second is the
    // end point (8.5.2.2).
    let (_, events) = run("0 0 m 1 2 3 4 v 5 6 7 8 y 1 2 3 4 5 6 c S", "", &[]);
    let (path, _) = only_path(&events);
    assert_eq!(
        path.segments,
        [
            MoveTo(0.0, 0.0),
            CurveTo(0.0, 0.0, 1.0, 2.0, 3.0, 4.0),
            CurveTo(5.0, 6.0, 7.0, 8.0, 7.0, 8.0),
            CurveTo(1.0, 2.0, 3.0, 4.0, 5.0, 6.0),
        ]
    );
}

#[test]
fn v_starts_from_the_end_of_the_last_curve() {
    let (_, events) = run("0 0 m 1 1 2 2 3 3 c 9 9 4 4 v S", "", &[]);
    let (path, _) = only_path(&events);
    assert_eq!(path.segments[2], CurveTo(3.0, 3.0, 9.0, 9.0, 4.0, 4.0));
}

#[test]
fn h_returns_to_the_start_of_the_subpath() {
    // After h the current point is the subpath's start, so v begins there.
    let (_, events) = run("5 5 m 9 9 l h 1 1 2 2 v S", "", &[]);
    let (path, _) = only_path(&events);
    assert_eq!(path.segments[3], CurveTo(5.0, 5.0, 1.0, 1.0, 2.0, 2.0));
}

#[test]
fn a_segment_with_no_current_point_is_misplaced_and_skipped() {
    let (document, events) = run("5 5 l 0 0 m 1 1 l S", "", &[]);
    let (path, _) = only_path(&events);
    assert_eq!(path.segments, [MoveTo(0.0, 0.0), LineTo(1.0, 1.0)]);
    assert!(has(
        &document,
        |k| matches!(k, DiagnosticKind::OperatorMisplaced { operator } if operator.as_bytes() == b"l")
    ));
}

#[test]
fn h_with_no_subpath_is_misplaced() {
    let (document, _) = run("h", "", &[]);
    assert!(has(&document, |k| matches!(
        k,
        DiagnosticKind::OperatorMisplaced { .. }
    )));
}

#[test]
fn n_ends_the_path_without_painting_it() {
    let (_, events) = run("0 0 m 5 5 l n 1 1 m 2 2 l S", "", &[]);
    let (path, _) = only_path(&events);
    assert_eq!(path.segments, [MoveTo(1.0, 1.0), LineTo(2.0, 2.0)]);
}

#[test]
fn painting_an_empty_path_sends_nothing() {
    let (document, events) = run("S f n", "", &[]);
    assert!(events.is_empty());
    assert!(document.diagnostics().is_empty());
}

#[test]
fn w_n_clips_without_painting() {
    let (_, events) = run("0 0 10 10 re W n", "", &[]);
    assert_eq!(events.len(), 1);
    let Event::Clip { path, rule } = &events[0] else {
        panic!()
    };
    assert_eq!(*rule, FillRule::NonZeroWinding);
    assert_eq!(path.segments.len(), 5);
}

#[test]
fn w_star_clips_by_the_even_odd_rule() {
    let (_, events) = run("0 0 10 10 re W* n", "", &[]);
    assert!(matches!(
        events[0],
        Event::Clip {
            rule: FillRule::EvenOdd,
            ..
        }
    ));
}

#[test]
fn a_clip_takes_effect_after_the_painting_of_its_own_path() {
    // 8.5.4: the path is painted first and then clips what follows.
    let (_, events) = run("0 0 10 10 re W f", "", &[]);
    assert!(matches!(events[0], Event::Path { .. }));
    assert!(matches!(events[1], Event::Clip { .. }));
}

#[test]
fn the_clip_does_not_outlive_its_path() {
    let (_, events) = run("0 0 10 10 re W n 0 0 5 5 re f", "", &[]);
    assert_eq!(events.len(), 2);
    assert!(matches!(events[1], Event::Path { .. }));
}

#[test]
fn w_with_operands_is_invalid() {
    let (document, events) = run("0 0 10 10 re 1 W n", "", &[]);
    assert!(events.is_empty());
    assert!(has(&document, |k| matches!(
        k,
        DiagnosticKind::OperandsInvalid { .. }
    )));
}

#[test]
fn a_path_is_in_user_space_and_the_ctm_is_sent_with_it() {
    let (_, events) = run("2 0 0 2 0 0 cm 1 1 m 2 2 l S", "", &[]);
    let Event::Path { path, state, .. } = paths(&events)[0] else {
        panic!()
    };
    assert_eq!(path.segments[0], MoveTo(1.0, 1.0));
    assert_eq!(state.ctm.a, 2.0);
}
