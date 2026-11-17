// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! TrueType fonts through the common interface.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::sfnt::{self, Glyph, SfntSpec};
use kernkraft_font::{CmapSelector, FontError, Format, GlyphId, Point, Program, Segment};

/// The contours of an outline with a line back to a contour's first point
/// dropped: `Close` already draws it, so the two forms are one shape.
fn closed(segments: Vec<Segment>) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    let mut start = None;
    for s in segments {
        match s {
            Segment::MoveTo(p) => start = Some(p),
            Segment::Close => {
                if let (Some(Segment::LineTo(last)), Some(first)) = (out.last(), start) {
                    if *last == first {
                        out.pop();
                    }
                }
            }
            _ => {}
        }
        out.push(s);
    }
    out
}

fn font() -> Vec<u8> {
    let spec = SfntSpec {
        unicode: vec![(0x41, 1), (0x20, 2), (0x42, 3)],
        mac: vec![(0x41, 1), (0x80, 3)],
        ..SfntSpec::new(vec![
            Glyph {
                name: ".notdef",
                advance: 500,
                contours: vec![],
            },
            Glyph {
                name: "tri",
                advance: 700,
                contours: sfnt::triangle(),
            },
            Glyph {
                name: "blank",
                advance: 250,
                contours: vec![],
            },
            Glyph {
                name: "arc",
                advance: 650,
                contours: sfnt::arc(),
            },
        ])
    };
    sfnt::build(&spec)
}

#[test]
fn format_count_matrix_and_advances() {
    let bytes = font();
    let prog = Program::parse(&bytes, 0).unwrap();
    assert_eq!(prog.format(), Format::TrueType);
    assert_eq!(prog.glyph_count(), 4);
    assert_eq!(prog.font_matrix(), [0.001, 0.0, 0.0, 0.001, 0.0, 0.0]);
    assert_eq!(prog.advance(GlyphId(0)), Some(500.0));
    assert_eq!(prog.advance(GlyphId(1)), Some(700.0));
    assert_eq!(prog.advance(GlyphId(2)), Some(250.0));
    assert_eq!(prog.advance(GlyphId(3)), Some(650.0));
}

#[test]
fn outlines_are_derived_from_the_contour_points() {
    let bytes = font();
    let prog = Program::parse(&bytes, 0).unwrap();
    let p = Point::new;
    // Three on-curve points: a move, two lines, a closing segment.
    assert_eq!(
        closed(prog.outline(GlyphId(1)).unwrap().segments),
        vec![
            Segment::MoveTo(p(0.0, 0.0)),
            Segment::LineTo(p(100.0, 0.0)),
            Segment::LineTo(p(50.0, 100.0)),
            Segment::Close
        ]
    );
    // One off-curve point is the control point of one quadratic.
    assert_eq!(
        closed(prog.outline(GlyphId(3)).unwrap().segments),
        vec![
            Segment::MoveTo(p(0.0, 0.0)),
            Segment::QuadTo(p(50.0, 100.0), p(100.0, 0.0)),
            Segment::Close
        ]
    );
    // A glyph with no contours is empty, not an error.
    assert!(prog.outline(GlyphId(2)).unwrap().is_empty());
    assert!(prog.outline(GlyphId(0)).unwrap().is_empty());
    assert_eq!(
        prog.outline(GlyphId(4)),
        Err(FontError::NoSuchGlyph(GlyphId(4)))
    );
}

#[test]
fn control_box_covers_every_point() {
    let bytes = font();
    let prog = Program::parse(&bytes, 0).unwrap();
    assert_eq!(
        prog.outline(GlyphId(1)).unwrap().control_box(),
        Some([0.0, 0.0, 100.0, 100.0])
    );
    assert_eq!(prog.outline(GlyphId(2)).unwrap().control_box(), None);
}

#[test]
fn lookup_by_unicode_cmap_and_name() {
    let bytes = font();
    let prog = Program::parse(&bytes, 0).unwrap();
    assert_eq!(prog.glyph_by_unicode('A'), Some(GlyphId(1)));
    assert_eq!(prog.glyph_by_unicode(' '), Some(GlyphId(2)));
    assert_eq!(prog.glyph_by_unicode('Z'), None);
    let mac = CmapSelector {
        platform: 1,
        encoding: 0,
    };
    let win = CmapSelector {
        platform: 3,
        encoding: 1,
    };
    assert_eq!(prog.glyph_by_cmap(mac, 0x80), Some(GlyphId(3)));
    assert_eq!(prog.glyph_by_cmap(mac, 0x42), None);
    assert_eq!(prog.glyph_by_cmap(win, 0x42), Some(GlyphId(3)));
    assert_eq!(
        prog.glyph_by_cmap(
            CmapSelector {
                platform: 3,
                encoding: 0
            },
            0x41
        ),
        None
    );
    assert_eq!(prog.glyph_by_name("arc"), Some(GlyphId(3)));
    assert_eq!(prog.glyph_name(GlyphId(1)).as_deref(), Some("tri"));
    assert_eq!(prog.glyph_by_name("nope"), None);
    // A TrueType program has no built-in one-byte encoding and no CIDs.
    assert_eq!(prog.glyph_by_builtin_code(65), None);
    assert_eq!(prog.glyph_by_cid(1), None);
}

#[test]
fn names_and_metrics() {
    let bytes = font();
    let prog = Program::parse(&bytes, 0).unwrap();
    let n = prog.names();
    assert_eq!(n.family.as_deref(), Some("Test Sans"));
    assert_eq!(n.subfamily.as_deref(), Some("Regular"));
    assert_eq!(n.full.as_deref(), Some("Test Sans Regular"));
    assert_eq!(n.postscript.as_deref(), Some("TestSans-Regular"));
    let m = prog.metrics();
    assert_eq!(m.ascent, Some(800.0));
    assert_eq!(m.descent, Some(-200.0));
    assert_eq!(m.cap_height, Some(700.0));
    assert_eq!(m.x_height, Some(500.0));
    assert_eq!(m.weight, Some(400));
    assert_eq!(m.is_italic, Some(false));
    assert_eq!(m.is_monospaced, Some(false));
    assert_eq!(m.italic_angle, Some(0.0));
}

#[test]
fn italic_bold_and_monospace_come_from_the_font() {
    let spec = SfntSpec {
        weight: 700,
        italic: true,
        fixed_pitch: true,
        italic_angle_tenths: -120,
        ..SfntSpec::new(vec![Glyph {
            name: ".notdef",
            advance: 600,
            contours: vec![],
        }])
    };
    let bytes = sfnt::build(&spec);
    let m = Program::parse(&bytes, 0).unwrap().metrics();
    assert_eq!(m.weight, Some(700));
    assert_eq!(m.is_italic, Some(true));
    assert_eq!(m.is_monospaced, Some(true));
    assert_eq!(m.italic_angle, Some(-12.0));
}

#[test]
fn units_per_em_sets_the_matrix() {
    let spec = SfntSpec {
        units_per_em: 2048,
        ..SfntSpec::new(vec![Glyph {
            name: ".notdef",
            advance: 600,
            contours: vec![],
        }])
    };
    let bytes = sfnt::build(&spec);
    let prog = Program::parse(&bytes, 0).unwrap();
    assert_eq!(prog.font_matrix()[0], 1.0 / 2048.0);
}

#[test]
fn a_collection_index_past_the_end() {
    // A one-font collection: header, then the one font after it.
    let font = font();
    let mut ttc = Vec::new();
    ttc.extend(*b"ttcf");
    ttc.extend(0x0001_0000u32.to_be_bytes());
    ttc.extend(1u32.to_be_bytes());
    ttc.extend(16u32.to_be_bytes());
    // Table offsets in the font are relative to the file start, so move each.
    let shift = 16u32;
    let mut moved = font.clone();
    let count = u16::from_be_bytes([moved[4], moved[5]]) as usize;
    for i in 0..count {
        let at = 12 + i * 16 + 8;
        let off =
            u32::from_be_bytes([moved[at], moved[at + 1], moved[at + 2], moved[at + 3]]) + shift;
        moved[at..at + 4].copy_from_slice(&off.to_be_bytes());
    }
    ttc.extend(moved);
    assert!(Program::parse(&ttc, 0).is_ok());
    assert_eq!(
        Program::parse(&ttc, 1).err(),
        Some(FontError::CollectionIndex { index: 1, count: 1 })
    );
}

#[test]
fn damaged_files_are_typed_errors_never_panics() {
    let bytes = font();
    for cut in 0..bytes.len() {
        let _ = Program::parse(&bytes[..cut], 0);
    }
    for at in (0..bytes.len()).step_by(7) {
        let mut damaged = bytes.clone();
        damaged[at] ^= 0xA5;
        if let Ok(prog) = Program::parse(&damaged, 0) {
            for g in 0..prog.glyph_count().min(64) {
                let _ = prog.outline(GlyphId(g));
                let _ = prog.advance(GlyphId(g));
            }
            let _ = prog.names();
            let _ = prog.metrics();
        }
    }
    assert!(matches!(
        Program::parse(&[0, 1, 0, 0], 0),
        Err(FontError::Sfnt(_))
    ));
}

#[test]
fn a_damaged_glyph_is_an_error_and_an_empty_one_is_not() {
    let mut bytes = font();
    let (glyf, _) = sfnt::table(&bytes, b"glyf").unwrap();
    // The triangle is the first glyph with data, so it starts the table:
    // numberOfContours (2 bytes), a box (8), then endPtsOfContours. Point the
    // last point of the one contour far past the end of the glyph.
    bytes[glyf + 10..glyf + 12].copy_from_slice(&0xFFF0u16.to_be_bytes());
    let prog = Program::parse(&bytes, 0).unwrap();
    assert!(
        matches!(
            prog.outline(GlyphId(1)),
            Err(FontError::Outline {
                glyph: GlyphId(1),
                ..
            })
        ),
        "{:?}",
        prog.outline(GlyphId(1))
    );
    // The glyphs with no contours still give empty outlines.
    assert!(prog.outline(GlyphId(2)).unwrap().is_empty());
}

#[test]
fn a_font_with_no_vector_outlines_says_so() {
    let mut bytes = font();
    // Rename the glyf table so the font has outlines of no kind.
    let count = u16::from_be_bytes([bytes[4], bytes[5]]) as usize;
    let at = (0..count)
        .map(|i| 12 + i * 16)
        .find(|&r| &bytes[r..r + 4] == b"glyf")
        .unwrap();
    bytes[at..at + 4].copy_from_slice(b"gly2");
    let prog = Program::parse(&bytes, 0).unwrap();
    assert_eq!(prog.format(), Format::SfntNoOutlines);
    assert_eq!(
        prog.outline(GlyphId(1)),
        Err(FontError::NoOutline(GlyphId(1)))
    );
    // Widths and the cmap still work: the font is usable for text, not for shapes.
    assert_eq!(prog.advance(GlyphId(1)), Some(700.0));
    assert_eq!(prog.glyph_by_unicode('A'), Some(GlyphId(1)));
}
