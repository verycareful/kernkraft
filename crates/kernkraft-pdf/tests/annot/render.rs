// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The appearance streams the library writes, drawn by the renderer: what
//! a highlight does to the page below it (a multiply, 11.3.5.2 of ISO
//! 32000-1), where an ink stroke and its round caps fall, and where a note
//! icon hangs on a page turned a quarter. Pixels at one pixel to the point,
//! `y` growing down: user `(x, y)` on the 612 by 792 page is pixel
//! `(x, 792 - y)`.

use super::*;
use kernkraft_fs::NoProgress;
use kernkraft_pdf::colour::Rgb;
use kernkraft_pdf::doc::Document;
use kernkraft_pdf::render::{render_page, RenderOptions};
use kernkraft_pdf::{add_annotation, edit_annotation, AnnotationEdit, NewAnnotation, NewShape};

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

/// A cyan box, user `[100 600 300 700]`.
const CYAN_BOX: &str = "0 1 1 rg 100 600 200 100 re f";

pub fn pixel(document: &Document, x: u32, y: u32) -> [u8; 4] {
    let raster = render_page(
        document,
        1,
        &RenderOptions {
            scale: 1.0,
            ..Default::default()
        },
        &NoProgress,
    )
    .unwrap();
    let at = ((y * raster.width + x) * 4) as usize;
    raster.rgba[at..at + 4].try_into().unwrap()
}

/// The pixels of one render, by `(x, y)`.
fn pixels(document: &Document, at: &[(u32, u32)]) -> Vec<[u8; 4]> {
    let raster = render_page(
        document,
        1,
        &RenderOptions {
            scale: 1.0,
            ..Default::default()
        },
        &NoProgress,
    )
    .unwrap();
    at.iter()
        .map(|&(x, y)| {
            let i = ((y * raster.width + x) * 4) as usize;
            raster.rgba[i..i + 4].try_into().unwrap()
        })
        .collect()
}

fn new(shape: NewShape, colour: Rgb, opacity: f64) -> NewAnnotation {
    NewAnnotation {
        shape,
        colour,
        opacity,
        contents: None,
        author: None,
        modified: WHEN,
    }
}

/// The box user `[100 650 400 700]`: displayed 100 across, 92 down, 300
/// wide and 50 high.
fn highlight_box() -> Rect {
    points_rect(100.0, 92.0, 400.0, 142.0)
}

fn highlighted(opacity: f64) -> (Document, u32) {
    let fixture = document(CYAN_BOX, "", "", &[]);
    let added = add_annotation(
        &fixture.open(),
        1,
        &new(
            NewShape::Highlight {
                quads: vec![highlight_box()],
            },
            YELLOW,
            opacity,
        ),
    )
    .unwrap();
    (added.document, added.id.number)
}

fn close(actual: [u8; 4], expected: [u8; 3], tolerance: i32, what: &str) {
    for (channel, (a, e)) in actual.iter().zip(expected).enumerate() {
        assert!(
            (i32::from(*a) - i32::from(e)).abs() <= tolerance,
            "{what}: {actual:?} is not {expected:?} (channel {channel})"
        );
    }
    assert_eq!(actual[3], 255, "{what}: alpha");
}

#[test]
fn a_highlight_multiplies_its_colour_with_the_page() {
    let (document, _) = highlighted(1.0);
    // Inside the box and the cyan: cyan times yellow is green (0 1 1 x
    // 1 1 0 = 0 1 0). Inside the box and on white: yellow. Outside the box:
    // the page as it was.
    let got = pixels(&document, &[(150, 120), (350, 120), (50, 120), (150, 150)]);
    close(got[0], [0, 255, 0], 1, "on the cyan");
    close(got[1], [255, 255, 0], 1, "on white");
    close(got[2], [255, 255, 255], 0, "left of the box");
    // Below the box (user y 640) the cyan is as it was.
    close(got[3], [0, 255, 255], 0, "below the box");
}

#[test]
fn a_half_opaque_highlight_is_half_of_the_way() {
    let (document, _) = highlighted(0.5);
    // Half of white and half of yellow: (255, 255, 127.5).
    close(
        pixels(&document, &[(350, 120)])[0],
        [255, 255, 128],
        2,
        "on white",
    );
    // Half of cyan and half of green: (0, 255, 127.5).
    close(
        pixels(&document, &[(150, 120)])[0],
        [0, 255, 128],
        2,
        "on cyan",
    );
}

#[test]
fn a_recoloured_highlight_is_drawn_in_the_new_colour() {
    let (document, id_number) = highlighted(1.0);
    let edited = edit_annotation(
        &document,
        1,
        id(id_number),
        &AnnotationEdit {
            colour: Some(Rgb {
                r: 1.0,
                g: 0.0,
                b: 1.0,
            }),
            ..Default::default()
        },
        &WHEN,
    )
    .unwrap();
    // Cyan times magenta is blue; white times magenta is magenta.
    let got = pixels(&edited, &[(150, 120), (350, 120)]);
    close(got[0], [0, 0, 255], 1, "on the cyan");
    close(got[1], [255, 0, 255], 1, "on white");
}

#[test]
fn a_moved_highlight_is_drawn_where_it_went() {
    let (document, id_number) = highlighted(1.0);
    // 50 points down the page: the fraction 50 / 792.
    let edited = edit_annotation(
        &document,
        1,
        id(id_number),
        &AnnotationEdit {
            translate: Some((0.0, 50.0 / PAGE_HEIGHT)),
            ..Default::default()
        },
        &WHEN,
    )
    .unwrap();
    // The box now spans displayed rows 142 to 192: row 120 is the page
    // again, row 170 (user y 622) is inside it, on the cyan.
    let got = pixels(&edited, &[(350, 120), (150, 170), (350, 170)]);
    close(got[0], [255, 255, 255], 0, "where it was");
    close(got[1], [0, 255, 0], 1, "where it is, on the cyan");
    close(got[2], [255, 255, 0], 1, "where it is, on white");
}

#[test]
fn an_ink_stroke_has_its_width_and_round_caps_and_a_point_is_a_dot() {
    let fixture = blank();
    let added = add_annotation(
        &fixture.open(),
        1,
        &new(
            NewShape::Ink {
                strokes: vec![
                    vec![point(100.0, 392.0), point(200.0, 392.0)],
                    vec![point(300.0, 300.0)],
                ],
                width: 4.0,
            },
            RED,
            1.0,
        ),
    )
    .unwrap();
    // The stroke covers displayed rows 390 to 394 from x 100 to 200, with
    // a round cap of radius 2 at each end; the point is a dot of radius 2.
    let got = pixels(
        &added.document,
        &[
            (150, 391),
            (150, 392),
            (150, 380),
            (99, 391),
            (96, 391),
            (299, 299),
            (300, 300),
            (310, 300),
        ],
    );
    close(got[0], [255, 0, 0], 0, "on the stroke");
    close(got[1], [255, 0, 0], 0, "on the stroke");
    close(got[2], [255, 255, 255], 0, "above it");
    close(got[3], [255, 0, 0], 0, "in the cap");
    close(got[4], [255, 255, 255], 0, "past the cap");
    close(got[5], [255, 0, 0], 0, "in the dot");
    close(got[6], [255, 0, 0], 0, "in the dot");
    close(got[7], [255, 255, 255], 0, "beside the dot");
}

#[test]
fn a_note_icon_hangs_from_its_point_with_an_outline() {
    let fixture = blank();
    let fill = Rgb {
        r: 1.0,
        g: 0.8,
        b: 0.0,
    };
    let added = add_annotation(
        &fixture.open(),
        1,
        &new(
            NewShape::Note {
                at: point(306.0, 396.0),
            },
            fill,
            1.0,
        ),
    )
    .unwrap();
    // The icon fills displayed x 306 to 326 and y 396 to 416. Its outline
    // is 1 point wide inside that box; the text lines lie 6, 10 and 14
    // points from the bottom, from 4 to 16 across.
    let got = pixels(
        &added.document,
        &[
            (309, 399),
            (306, 405),
            (325, 405),
            (316, 396),
            (305, 405),
            (327, 405),
        ],
    );
    close(got[0], [255, 204, 0], 0, "the fill");
    close(got[1], [0, 0, 0], 0, "the left outline");
    close(got[2], [0, 0, 0], 0, "the right outline");
    close(got[3], [0, 0, 0], 0, "the top outline");
    close(got[4], [255, 255, 255], 0, "left of the icon");
    close(got[5], [255, 255, 255], 0, "right of the icon");
}

#[test]
fn a_note_on_a_rotated_page_is_drawn_upright_at_its_point() {
    // Displayed 792 wide and 612 high. The point is displayed (198, 306).
    let fixture = document("", "/Rotate 90", "", &[]);
    let fill = Rgb {
        r: 1.0,
        g: 0.8,
        b: 0.0,
    };
    let added = add_annotation(
        &fixture.open(),
        1,
        &new(
            NewShape::Note {
                at: (198.0 / 792.0, 306.0 / 612.0),
            },
            fill,
            1.0,
        ),
    )
    .unwrap();
    let got = pixels(
        &added.document,
        &[(201, 309), (198, 309), (197, 309), (219, 309)],
    );
    close(got[0], [255, 204, 0], 0, "the fill");
    close(got[1], [0, 0, 0], 0, "the outline");
    close(got[2], [255, 255, 255], 0, "left of the icon");
    close(got[3], [255, 255, 255], 0, "right of the icon");
}

#[test]
fn an_underline_is_a_line_along_the_bottom_of_the_box() {
    let fixture = blank();
    let added = add_annotation(
        &fixture.open(),
        1,
        &new(
            NewShape::Underline {
                quads: vec![points_rect(100.0, 92.0, 400.0, 142.0)],
            },
            RED,
            1.0,
        ),
    )
    .unwrap();
    // The line is on displayed y 142, half a point either side; at one
    // pixel to the point each of rows 141 and 142 is half covered, so the
    // red is half red over white. Nothing is drawn above, and nothing
    // left of the box.
    let got = pixels(
        &added.document,
        &[(200, 141), (200, 142), (200, 120), (200, 150), (90, 141)],
    );
    close(got[0], [255, 128, 128], 2, "above the line");
    close(got[1], [255, 128, 128], 2, "below the line");
    close(got[2], [255, 255, 255], 0, "in the box");
    close(got[3], [255, 255, 255], 0, "under the line");
    close(got[4], [255, 255, 255], 0, "left of the box");
}
