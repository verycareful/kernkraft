// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! `edit_annotation`: what an edit changes, what it keeps, and what it
//! refuses. ISO 32000-1, 12.5.2 (Table 164), 12.5.5 and 12.5.6.

use super::*;
use kernkraft_pdf::colour::Rgb;
use kernkraft_pdf::syntax::Value;
use kernkraft_pdf::{edit_annotation, AnnotationEdit, PdfError};

/// A highlight (50) written by another program: a red 60 by 20 box,
/// half opaque, with an entry this library does not know (`X`), a rollover
/// appearance (71) beside the normal one (70), and a comment.
fn foreign_highlight() -> Fixture {
    let blue = stream(
        " /Type /XObject /Subtype /Form /BBox [100 700 160 720]",
        b"0 0 1 rg 100 700 60 20 re f",
    );
    let green = stream(
        " /Type /XObject /Subtype /Form /BBox [100 700 160 720]",
        b"0 1 0 rg 100 700 60 20 re f",
    );
    annotated(
        "",
        &[50],
        &[
            (
                50,
                b"<< /Type /Annot /Subtype /Highlight /Rect [100 700 160 720] \
                  /QuadPoints [100 720 160 720 100 700 160 700] /C [1 0 0] /CA 0.5 \
                  /Contents (old) /T (me) /X (keep) /AP << /N 70 0 R /R 71 0 R >> >>"
                    .as_slice(),
            ),
            (70, blue.as_slice()),
            (71, green.as_slice()),
        ],
    )
}

fn edit(changes: AnnotationEdit, fixture: &Fixture, number: u32) -> kernkraft_pdf::doc::Document {
    edit_annotation(&fixture.open(), 1, id(number), &changes, &WHEN).unwrap()
}

#[test]
fn a_comment_edit_changes_the_comment_and_the_date_and_nothing_else() {
    let fixture = foreign_highlight();
    let document = edit(
        AnnotationEdit {
            contents: Some("new".to_string()),
            ..Default::default()
        },
        &fixture,
        50,
    );
    let annotation = dict(&document, 50);
    assert_eq!(
        string_of(&document, &annotation, "Contents"),
        Some(b"new".to_vec())
    );
    assert_eq!(
        string_of(&document, &annotation, "M"),
        Some(WHEN_TEXT.to_vec())
    );
    // Everything else, the entry nobody here knows included, is as it was.
    assert_eq!(
        string_of(&document, &annotation, "X"),
        Some(b"keep".to_vec())
    );
    assert_eq!(string_of(&document, &annotation, "T"), Some(b"me".to_vec()));
    assert_numbers(&numbers(&document, &annotation, "C"), &[1.0, 0.0, 0.0], "C");
    assert_eq!(real_of(&document, &annotation, "CA"), Some(0.5));
    assert_eq!(appearance_of(&document, 50), 70);
    // Nothing was drawn again, so no object was made.
    assert_eq!(
        document.next_object_number(),
        fixture.open().next_object_number()
    );
}

#[test]
fn an_empty_text_takes_the_entry_away() {
    let fixture = foreign_highlight();
    let document = edit(
        AnnotationEdit {
            contents: Some(String::new()),
            author: Some(String::new()),
            ..Default::default()
        },
        &fixture,
        50,
    );
    let annotation = dict(&document, 50);
    assert!(annotation.get(b"Contents").is_none());
    assert!(annotation.get(b"T").is_none());
}

#[test]
fn a_new_colour_draws_the_appearance_again_and_keeps_the_rest_of_ap() {
    let fixture = foreign_highlight();
    let document = edit(
        AnnotationEdit {
            colour: Some(Rgb {
                r: 0.0,
                g: 1.0,
                b: 0.0,
            }),
            ..Default::default()
        },
        &fixture,
        50,
    );
    let annotation = dict(&document, 50);
    assert_numbers(&numbers(&document, &annotation, "C"), &[0.0, 1.0, 0.0], "C");
    let normal = appearance_of(&document, 50);
    assert_ne!(normal, 70, "the appearance is a new stream");
    // The colour is the new one; the opacity the file had stays (CA 0.5).
    assert_eq!(
        stream_text(&document, normal),
        "q\n/GS gs\n0 1 0 rg\n100 720 m\n160 720 l\n160 700 l\n100 700 l\nh\nf\nQ\n"
    );
    let state = graphics_state(&document, normal);
    assert_eq!(real_of(&document, &state, "ca"), Some(0.5));
    // The rollover appearance is untouched, and so is the old normal one.
    let ap = document.get(&annotation, b"AP").unwrap();
    match ap.dictionary().unwrap().get(b"R").unwrap().value() {
        Value::Reference(rollover) => assert_eq!(rollover.number, 71),
        other => panic!("R is {other:?}"),
    }
    assert_eq!(stream_text(&document, 70), "0 0 1 rg 100 700 60 20 re f");
}

#[test]
fn opacity_is_set_and_one_takes_the_entry_away() {
    let fixture = foreign_highlight();
    let document = edit(
        AnnotationEdit {
            opacity: Some(0.25),
            ..Default::default()
        },
        &fixture,
        50,
    );
    let annotation = dict(&document, 50);
    assert_eq!(real_of(&document, &annotation, "CA"), Some(0.25));
    let state = graphics_state(&document, appearance_of(&document, 50));
    assert_eq!(real_of(&document, &state, "CA"), Some(0.25));
    assert_eq!(real_of(&document, &state, "ca"), Some(0.25));

    let document = edit(
        AnnotationEdit {
            opacity: Some(1.0),
            ..Default::default()
        },
        &fixture,
        50,
    );
    assert!(dict(&document, 50).get(b"CA").is_none());
}

#[test]
fn a_move_shifts_the_box_the_quads_and_the_appearance() {
    let fixture = foreign_highlight();
    let document = edit(
        AnnotationEdit {
            translate: Some((0.1, 0.25)),
            ..Default::default()
        },
        &fixture,
        50,
    );
    // 10% of 612 across, 25% of 792 down: 61.2 to the right, 198 lower.
    let annotation = dict(&document, 50);
    let close = |actual: &[f64], expected: &[f64], what: &str| {
        assert_eq!(actual.len(), expected.len(), "{what}");
        for (a, e) in actual.iter().zip(expected) {
            assert!((a - e).abs() < 1e-6, "{what}: {a} is not {e}");
        }
    };
    close(
        &numbers(&document, &annotation, "Rect"),
        &[161.2, 502.0, 221.2, 522.0],
        "Rect",
    );
    close(
        &numbers(&document, &annotation, "QuadPoints"),
        &[161.2, 522.0, 221.2, 522.0, 161.2, 502.0, 221.2, 502.0],
        "QuadPoints",
    );
    let form = dict(&document, appearance_of(&document, 50));
    close(
        &numbers(&document, &form, "BBox"),
        &[161.2, 502.0, 221.2, 522.0],
        "BBox",
    );
    // What is listed is the old place moved by what was asked.
    let before = fixture.open().annotations(1).unwrap();
    let after = document.annotations(1).unwrap();
    assert_close(after[0].rect.left, before[0].rect.left + 0.1, "left");
    assert_close(after[0].rect.top, before[0].rect.top + 0.25, "top");
}

#[test]
fn ink_takes_a_new_width_and_keeps_the_rest_of_its_border_style() {
    let fixture = annotated(
        "",
        &[50],
        &[
            (
                50,
                b"<< /Type /Annot /Subtype /Ink /Rect [90 390 210 410] /C [0 0 1] \
                  /InkList [[100 400 200 400]] /BS 80 0 R >>"
                    .as_slice(),
            ),
            (80, b"<< /W 2 /S /D /D [3 2] >>".as_slice()),
        ],
    );
    let document = edit(
        AnnotationEdit {
            line_width: Some(6.0),
            ..Default::default()
        },
        &fixture,
        50,
    );
    let annotation = dict(&document, 50);
    let style = document.get(&annotation, b"BS").unwrap();
    let style = style.dictionary().unwrap();
    assert_eq!(real_of(&document, style, "W"), Some(6.0));
    assert_eq!(name_of(&document, style, "S").as_deref(), Some("D"));
    assert_numbers(&numbers(&document, style, "D"), &[3.0, 2.0], "D");
    assert_eq!(
        stream_text(&document, appearance_of(&document, 50)),
        "q\n/GS gs\n0 0 1 RG\n6 w\n1 J\n1 j\n100 400 m\n200 400 l\nS\nQ\n"
    );
}

#[test]
fn a_line_width_is_for_ink_only() {
    let fixture = foreign_highlight();
    let result = edit_annotation(
        &fixture.open(),
        1,
        id(50),
        &AnnotationEdit {
            line_width: Some(2.0),
            ..Default::default()
        },
        &WHEN,
    );
    assert!(matches!(
        result,
        Err(PdfError::AnnotationNotEditable { ref subtype }) if subtype == "Highlight"
    ));
}

#[test]
fn what_the_library_does_not_draw_can_be_commented_but_not_recoloured() {
    let fixture = annotated(
        "",
        &[50],
        &[(
            50,
            b"<< /Type /Annot /Subtype /Square /Rect [0 0 10 10] /C [1 0 0] >>".as_slice(),
        )],
    );
    let document = edit(
        AnnotationEdit {
            contents: Some("a square".to_string()),
            ..Default::default()
        },
        &fixture,
        50,
    );
    assert_eq!(
        string_of(&document, &dict(&document, 50), "Contents"),
        Some(b"a square".to_vec())
    );
    for changes in [
        AnnotationEdit {
            colour: Some(Rgb {
                r: 0.0,
                g: 0.0,
                b: 0.0,
            }),
            ..Default::default()
        },
        AnnotationEdit {
            opacity: Some(0.5),
            ..Default::default()
        },
        AnnotationEdit {
            translate: Some((0.1, 0.1)),
            ..Default::default()
        },
    ] {
        let result = edit_annotation(&fixture.open(), 1, id(50), &changes, &WHEN);
        assert!(
            matches!(
                result,
                Err(PdfError::AnnotationNotEditable { ref subtype }) if subtype == "Square"
            ),
            "{changes:?}"
        );
    }
}

#[test]
fn an_annotation_the_page_does_not_list_is_refused() {
    let fixture = annotated(
        "",
        &[],
        &[(
            50,
            b"<< /Type /Annot /Subtype /Text /Rect [0 0 10 10] >>".as_slice(),
        )],
    );
    let result = edit_annotation(
        &fixture.open(),
        1,
        id(50),
        &AnnotationEdit::default(),
        &WHEN,
    );
    assert!(matches!(
        result,
        Err(PdfError::AnnotationNotOnPage { page: 1, .. })
    ));
}

#[test]
fn a_markup_with_no_quads_cannot_be_drawn_again() {
    let fixture = annotated(
        "",
        &[50],
        &[(
            50,
            b"<< /Type /Annot /Subtype /Highlight /Rect [0 0 10 10] /C [1 0 0] >>".as_slice(),
        )],
    );
    let result = edit_annotation(
        &fixture.open(),
        1,
        id(50),
        &AnnotationEdit {
            opacity: Some(0.5),
            ..Default::default()
        },
        &WHEN,
    );
    assert!(matches!(
        result,
        Err(PdfError::InvalidEntry {
            key: "QuadPoints",
            ..
        })
    ));
}

#[test]
fn an_annotation_with_no_colour_needs_one_to_be_drawn_again() {
    let fixture = annotated(
        "",
        &[50],
        &[(
            50,
            b"<< /Type /Annot /Subtype /Text /Rect [0 0 20 20] >>".as_slice(),
        )],
    );
    let result = edit_annotation(
        &fixture.open(),
        1,
        id(50),
        &AnnotationEdit {
            opacity: Some(0.5),
            ..Default::default()
        },
        &WHEN,
    );
    assert!(matches!(result, Err(PdfError::Invalid(_))));
    // Given one, it is drawn.
    let document = edit(
        AnnotationEdit {
            colour: Some(Rgb {
                r: 1.0,
                g: 1.0,
                b: 0.0,
            }),
            ..Default::default()
        },
        &fixture,
        50,
    );
    assert!(dict(&document, 50).get(b"AP").is_some());
}

#[test]
fn values_out_of_range_are_refused() {
    let fixture = foreign_highlight();
    for changes in [
        AnnotationEdit {
            opacity: Some(2.0),
            ..Default::default()
        },
        AnnotationEdit {
            colour: Some(Rgb {
                r: 2.0,
                g: 0.0,
                b: 0.0,
            }),
            ..Default::default()
        },
        AnnotationEdit {
            translate: Some((f64::NAN, 0.0)),
            ..Default::default()
        },
        AnnotationEdit {
            line_width: Some(0.0),
            ..Default::default()
        },
    ] {
        let result = edit_annotation(&fixture.open(), 1, id(50), &changes, &WHEN);
        assert!(matches!(result, Err(PdfError::Invalid(_))), "{changes:?}");
    }
}
