// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! `add_annotation`: the entries and the appearance stream each kind is
//! written with. ISO 32000-1, 12.5.2 (Tables 164 and 165), 12.5.6.4,
//! 12.5.6.10 and 12.5.6.13.

use super::*;
use kernkraft_pdf::colour::Rgb;
use kernkraft_pdf::syntax::Value;
use kernkraft_pdf::{add_annotation, AnnotationKind, NewAnnotation, NewShape, PdfError};

const YELLOW: Rgb = Rgb {
    r: 1.0,
    g: 1.0,
    b: 0.0,
};
const RED: Rgb = Rgb {
    r: 1.0,
    g: 0.0,
    b: 0.0,
};

fn new(shape: NewShape, colour: Rgb) -> NewAnnotation {
    NewAnnotation {
        shape,
        colour,
        opacity: 1.0,
        contents: None,
        author: None,
        modified: WHEN,
    }
}

/// The 60 by 20 point box whose top left corner is 100 points from the left
/// and 72 from the top: the user space box `[100 700 160 720]`.
fn line_box() -> Rect {
    points_rect(100.0, 72.0, 160.0, 92.0)
}

#[test]
fn a_highlight_is_written_with_its_quad_and_a_multiplying_appearance() {
    let added = add_annotation(
        &blank().open(),
        1,
        &new(
            NewShape::Highlight {
                quads: vec![line_box()],
            },
            YELLOW,
        ),
    )
    .unwrap();
    let document = &added.document;
    let annotation = dict(document, added.id.number);

    assert_eq!(
        name_of(document, &annotation, "Subtype").as_deref(),
        Some("Highlight")
    );
    assert_eq!(
        name_of(document, &annotation, "Type").as_deref(),
        Some("Annot")
    );
    // Top left, top right, bottom left, bottom right, each (x, y).
    assert_numbers(
        &numbers(document, &annotation, "QuadPoints"),
        &[100.0, 720.0, 160.0, 720.0, 100.0, 700.0, 160.0, 700.0],
        "QuadPoints",
    );
    assert_numbers(
        &numbers(document, &annotation, "Rect"),
        &[100.0, 700.0, 160.0, 720.0],
        "Rect",
    );
    assert_numbers(&numbers(document, &annotation, "C"), &[1.0, 1.0, 0.0], "C");
    // 12.5.2, Table 165: Print is bit 3.
    assert_eq!(integer_of(document, &annotation, "F"), Some(4));
    assert!(
        annotation.get(b"CA").is_none(),
        "an opaque annotation has no CA"
    );
    assert_eq!(
        string_of(document, &annotation, "M"),
        Some(WHEN_TEXT.to_vec())
    );
    match annotation.get(b"P").unwrap().value() {
        Value::Reference(page) => assert_eq!(page.number, page_object(0)),
        other => panic!("P is {other:?}"),
    }

    let appearance = appearance_of(document, added.id.number);
    // The quad is filled round: top left, top right, bottom right, bottom
    // left.
    assert_eq!(
        stream_text(document, appearance),
        "q\n/GS gs\n1 1 0 rg\n100 720 m\n160 720 l\n160 700 l\n100 700 l\nh\nf\nQ\n"
    );
    let form = dict(document, appearance);
    assert_numbers(
        &numbers(document, &form, "BBox"),
        &[100.0, 700.0, 160.0, 720.0],
        "BBox",
    );
    assert_eq!(name_of(document, &form, "Subtype").as_deref(), Some("Form"));
    let state = graphics_state(document, appearance);
    assert_eq!(name_of(document, &state, "BM").as_deref(), Some("Multiply"));
    assert_eq!(real_of(document, &state, "ca"), Some(1.0));
}

#[test]
fn an_underline_and_a_strikeout_draw_lines_in_a_box_that_holds_them() {
    let under = add_annotation(
        &blank().open(),
        1,
        &new(
            NewShape::Underline {
                quads: vec![line_box()],
            },
            RED,
        ),
    )
    .unwrap();
    let document = &under.document;
    let annotation = dict(document, under.id.number);
    assert_eq!(
        name_of(document, &annotation, "Subtype").as_deref(),
        Some("Underline")
    );
    // The line is 1 point wide, so the box grows by that on every side to
    // hold it and its soft edge.
    assert_numbers(
        &numbers(document, &annotation, "Rect"),
        &[99.0, 699.0, 161.0, 721.0],
        "Rect",
    );
    assert_eq!(
        stream_text(document, appearance_of(document, under.id.number)),
        "q\n/GS gs\n1 0 0 RG\n1 w\n100 700 m\n160 700 l\nS\nQ\n"
    );
    let state = graphics_state(document, appearance_of(document, under.id.number));
    assert_eq!(name_of(document, &state, "BM").as_deref(), Some("Normal"));

    let strike = add_annotation(
        &blank().open(),
        1,
        &new(
            NewShape::StrikeOut {
                quads: vec![line_box()],
            },
            RED,
        ),
    )
    .unwrap();
    let document = &strike.document;
    assert_eq!(
        name_of(document, &dict(document, strike.id.number), "Subtype").as_deref(),
        Some("StrikeOut")
    );
    // Through the middle: y 710, halfway between 700 and 720.
    assert_eq!(
        stream_text(document, appearance_of(document, strike.id.number)),
        "q\n/GS gs\n1 0 0 RG\n1 w\n100 710 m\n160 710 l\nS\nQ\n"
    );
}

#[test]
fn several_quads_are_all_written() {
    let second = points_rect(100.0, 92.0, 130.0, 112.0);
    let added = add_annotation(
        &blank().open(),
        1,
        &new(
            NewShape::Highlight {
                quads: vec![line_box(), second],
            },
            YELLOW,
        ),
    )
    .unwrap();
    let document = &added.document;
    let annotation = dict(document, added.id.number);
    assert_eq!(numbers(document, &annotation, "QuadPoints").len(), 16);
    // The box holds both: 100 to 160 across, 680 to 720 up.
    assert_numbers(
        &numbers(document, &annotation, "Rect"),
        &[100.0, 680.0, 160.0, 720.0],
        "Rect",
    );
    let listed = document.annotations(1).unwrap();
    assert_eq!(listed[0].quads.len(), 2);
}

#[test]
fn ink_strokes_are_written_with_their_width_and_round_ends() {
    let strokes = vec![
        vec![point(100.0, 392.0), point(200.0, 392.0)],
        vec![point(300.0, 300.0)],
    ];
    let added = add_annotation(
        &blank().open(),
        1,
        &new(
            NewShape::Ink {
                strokes,
                width: 4.0,
            },
            RED,
        ),
    )
    .unwrap();
    let document = &added.document;
    let annotation = dict(document, added.id.number);
    assert_eq!(
        name_of(document, &annotation, "Subtype").as_deref(),
        Some("Ink")
    );
    let list = document.get(&annotation, b"InkList").unwrap();
    let list = list.array().unwrap();
    assert_eq!(list.len(), 2);
    // Page y 392 from the top is user y 400.
    let first: Vec<f64> = match document.resolve(&list[0]).unwrap().value() {
        Value::Array(items) => items
            .iter()
            .map(|n| match n.value() {
                Value::Integer(n) => *n as f64,
                Value::Real(r) => r.get(),
                other => panic!("{other:?}"),
            })
            .collect(),
        other => panic!("{other:?}"),
    };
    assert_numbers(&first, &[100.0, 400.0, 200.0, 400.0], "first stroke");
    // The points' box is 100 to 300 across and 400 to 492 up (the lone
    // point is 300 across and 300 down); the width makes it 4 points larger
    // each way.
    assert_numbers(
        &numbers(document, &annotation, "Rect"),
        &[96.0, 396.0, 304.0, 496.0],
        "Rect",
    );
    let style = document.get(&annotation, b"BS").unwrap();
    assert_eq!(
        real_of(document, style.dictionary().unwrap(), "W"),
        Some(4.0)
    );
    assert_eq!(
        stream_text(document, appearance_of(document, added.id.number)),
        "q\n/GS gs\n1 0 0 RG\n4 w\n1 J\n1 j\n100 400 m\n200 400 l\nS\n300 492 m\n300 492 l\nS\nQ\n"
    );
}

#[test]
fn a_note_is_a_20_point_icon_that_stays_upright_and_the_same_size() {
    let added = add_annotation(
        &blank().open(),
        1,
        &new(
            NewShape::Note {
                at: point(100.0, 72.0),
            },
            YELLOW,
        ),
    )
    .unwrap();
    let document = &added.document;
    let annotation = dict(document, added.id.number);
    assert_eq!(
        name_of(document, &annotation, "Subtype").as_deref(),
        Some("Text")
    );
    assert_eq!(
        name_of(document, &annotation, "Name").as_deref(),
        Some("Note")
    );
    // The icon hangs from the point: 100 across, 720 down to 700 up.
    assert_numbers(
        &numbers(document, &annotation, "Rect"),
        &[100.0, 700.0, 120.0, 720.0],
        "Rect",
    );
    // Print (4), NoZoom (8) and NoRotate (16), Table 165.
    assert_eq!(integer_of(document, &annotation, "F"), Some(28));
    let listed = document.annotations(1).unwrap();
    assert_eq!(listed[0].kind, AnnotationKind::Note);
    assert_rect(listed[0].rect, 100.0, 72.0, 120.0, 92.0, "the icon");
}

#[test]
fn contents_author_and_opacity_are_written_as_text_strings_and_ca() {
    let mut annotation = new(
        NewShape::Note {
            at: point(10.0, 10.0),
        },
        YELLOW,
    );
    annotation.contents = Some("check this".to_string());
    annotation.author = Some("Zoë".to_string());
    annotation.opacity = 0.5;
    let added = add_annotation(&blank().open(), 1, &annotation).unwrap();
    let document = &added.document;
    let dictionary = dict(document, added.id.number);
    assert_eq!(
        string_of(document, &dictionary, "Contents"),
        Some(b"check this".to_vec())
    );
    // `ë` is 0xEB in PDFDocEncoding (Annex D), so the text stays one byte
    // a character.
    assert_eq!(
        string_of(document, &dictionary, "T"),
        Some(b"Zo\xEB".to_vec())
    );
    assert_eq!(real_of(document, &dictionary, "CA"), Some(0.5));
    let state = graphics_state(document, appearance_of(document, added.id.number));
    assert_eq!(real_of(document, &state, "CA"), Some(0.5));
    assert_eq!(real_of(document, &state, "ca"), Some(0.5));
}

#[test]
fn a_new_annotation_is_listed_last_and_the_ones_before_it_stay() {
    let fixture = annotated(
        "",
        &[50],
        &[(
            50,
            b"<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] >>".as_slice(),
        )],
    );
    let added = add_annotation(
        &fixture.open(),
        1,
        &new(
            NewShape::Note {
                at: point(10.0, 10.0),
            },
            YELLOW,
        ),
    )
    .unwrap();
    assert_eq!(annots_of(&added.document), vec![50, added.id.number]);
}

#[test]
fn an_annots_array_in_an_object_of_its_own_is_the_one_that_changes() {
    let fixture = document(
        "",
        "/Annots 50 0 R",
        "",
        &[
            (50, b"[51 0 R]".as_slice()),
            (
                51,
                b"<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] >>".as_slice(),
            ),
        ],
    );
    let added = add_annotation(
        &fixture.open(),
        1,
        &new(
            NewShape::Note {
                at: point(10.0, 10.0),
            },
            YELLOW,
        ),
    )
    .unwrap();
    let document = &added.document;
    // The page still points at object 50, which now lists both.
    let page = dict(document, page_object(0));
    match page.get(b"Annots").unwrap().value() {
        Value::Reference(array) => assert_eq!(array.number, 50),
        other => panic!("Annots is {other:?}"),
    }
    assert_eq!(annots_of(document), vec![51, added.id.number]);
}

#[test]
fn nothing_but_the_page_and_the_new_objects_is_edited() {
    let fixture = blank();
    let before = fixture.open();
    let added = add_annotation(
        &before,
        1,
        &new(
            NewShape::Note {
                at: point(10.0, 10.0),
            },
            YELLOW,
        ),
    )
    .unwrap();
    assert!(!before.is_edited());
    assert!(added.document.is_edited());
    // The new annotation and its appearance take the next two numbers.
    let next = before.next_object_number();
    assert_eq!(added.id.number, next + 1);
    assert_eq!(appearance_of(&added.document, added.id.number), next);
}

#[test]
fn a_rotated_page_gives_the_quad_in_user_space() {
    // Turned 90 degrees clockwise the 612 by 792 page is displayed 792
    // wide and 612 high, and displayed (x, y) is user (y, x) (7.7.3.3).
    let fixture = document("", "/Rotate 90", "", &[]);
    let rect = Rect {
        left: 0.25,
        top: 0.5,
        right: 0.5,
        bottom: 0.75,
    };
    let added = add_annotation(
        &fixture.open(),
        1,
        &new(NewShape::Highlight { quads: vec![rect] }, YELLOW),
    )
    .unwrap();
    let document = &added.document;
    // Displayed corners (198, 306), (396, 306), (198, 459), (396, 459).
    assert_numbers(
        &numbers(document, &dict(document, added.id.number), "QuadPoints"),
        &[306.0, 198.0, 306.0, 396.0, 459.0, 198.0, 459.0, 396.0],
        "QuadPoints",
    );
    // What is listed is what was given.
    let listed = document.annotations(1).unwrap();
    assert_close(listed[0].quads[0].left, 0.25, "left");
    assert_close(listed[0].quads[0].top, 0.5, "top");
    assert_close(listed[0].quads[0].right, 0.5, "right");
    assert_close(listed[0].quads[0].bottom, 0.75, "bottom");
}

#[test]
fn a_note_on_a_rotated_page_stays_where_it_was_put() {
    let fixture = document("", "/Rotate 270", "", &[]);
    let at = (0.25, 0.5);
    let added = add_annotation(&fixture.open(), 1, &new(NewShape::Note { at }, YELLOW)).unwrap();
    let listed = added.document.annotations(1).unwrap();
    // The page is displayed 792 by 612: the icon hangs from the point and
    // is 20 points either way.
    assert_close(listed[0].rect.left, 0.25, "left");
    assert_close(listed[0].rect.top, 0.5, "top");
    assert_close(listed[0].rect.right, 0.25 + 20.0 / 792.0, "right");
    assert_close(listed[0].rect.bottom, 0.5 + 20.0 / 612.0, "bottom");
}

#[test]
fn what_cannot_be_drawn_is_refused_and_nothing_changes() {
    let document = blank().open();
    let note = || {
        new(
            NewShape::Note {
                at: point(10.0, 10.0),
            },
            YELLOW,
        )
    };
    let invalid = |annotation: NewAnnotation| {
        assert!(
            matches!(
                add_annotation(&document, 1, &annotation),
                Err(PdfError::Invalid(_))
            ),
            "{annotation:?}"
        );
    };
    let mut bad = note();
    bad.opacity = 1.5;
    invalid(bad);
    let mut bad = note();
    bad.opacity = f64::NAN;
    invalid(bad);
    let mut bad = note();
    bad.colour = Rgb {
        r: -0.1,
        g: 0.0,
        b: 0.0,
    };
    invalid(bad);
    invalid(new(NewShape::Highlight { quads: vec![] }, YELLOW));
    invalid(new(NewShape::Underline { quads: vec![] }, YELLOW));
    invalid(new(
        NewShape::Note {
            at: (f64::NAN, 0.0),
        },
        YELLOW,
    ));
    invalid(new(
        NewShape::Ink {
            strokes: vec![],
            width: 1.0,
        },
        YELLOW,
    ));
    invalid(new(
        NewShape::Ink {
            strokes: vec![vec![]],
            width: 1.0,
        },
        YELLOW,
    ));
    for width in [0.0, -1.0, f64::INFINITY] {
        invalid(new(
            NewShape::Ink {
                strokes: vec![vec![(0.5, 0.5)]],
                width,
            },
            YELLOW,
        ));
    }
    assert!(!document.is_edited());
}

#[test]
fn a_page_out_of_range_is_refused() {
    let document = blank().open();
    for page in [0, 2] {
        assert!(matches!(
            add_annotation(
                &document,
                page,
                &new(NewShape::Note { at: (0.5, 0.5) }, YELLOW)
            ),
            Err(PdfError::PageOutOfRange { .. })
        ));
    }
}
