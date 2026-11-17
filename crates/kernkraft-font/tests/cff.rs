// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Bare CFF font programs (the `FontFile3` of a PDF) through the common
//! interface.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::cff::{self, CffGlyph, CffSpec, Cs2};
use kernkraft_font::{FontError, Format, GlyphId, Point, Program, Segment};

const NOMINAL: i32 = 500;
const DEFAULT: i32 = 600;

fn name_keyed() -> Vec<u8> {
    // SIDs 1, 34, 35, 36 are the standard strings space, A, B, C: SID n is the
    // character with code n + 31 for 1 to 95 (Appendix A of the CFF note).
    let spec = CffSpec {
        glyphs: vec![
            CffGlyph {
                id: 0,
                code: Cs2::new().op("endchar"),
            },
            // width 700: the first operand is 700 - nominal(500) = 200
            CffGlyph {
                id: 34,
                code: Cs2::new()
                    .with(&[200, 100, 0], "rmoveto")
                    .with(&[300], "hlineto")
                    .with(&[300], "vlineto")
                    .with(&[-300], "hlineto")
                    .op("endchar"),
            },
            // no width operand: the default
            CffGlyph {
                id: 35,
                code: Cs2::new()
                    .with(&[10, 20], "rmoveto")
                    .with(&[10, 0, 20, 10, 0, 30], "rrcurveto")
                    .op("endchar"),
            },
            // width 250 given to endchar: 250 - 500
            CffGlyph {
                id: 1,
                code: Cs2::new().with(&[-250], "endchar"),
            },
            // a call of local subroutine 0 in a font with none
            CffGlyph {
                id: 36,
                code: Cs2::new().with(&[0], "callsubr").op("endchar"),
            },
        ],
        default_width: DEFAULT,
        nominal_width: NOMINAL,
        matrix: None,
        cid_keyed: false,
        encoding: None,
    };
    cff::build(&spec)
}

fn cid_keyed() -> Vec<u8> {
    let spec = CffSpec {
        glyphs: vec![
            CffGlyph {
                id: 0,
                code: Cs2::new().op("endchar"),
            },
            CffGlyph {
                id: 500,
                code: Cs2::new()
                    .with(&[5, 5], "rmoveto")
                    .with(&[50], "hlineto")
                    .op("endchar"),
            },
            CffGlyph {
                id: 3,
                code: Cs2::new()
                    .with(&[1, 2], "rmoveto")
                    .with(&[20], "vlineto")
                    .op("endchar"),
            },
        ],
        default_width: DEFAULT,
        nominal_width: NOMINAL,
        matrix: None,
        cid_keyed: true,
        encoding: None,
    };
    cff::build(&spec)
}

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

#[test]
fn a_bare_cff_program_is_recognised_and_counted() {
    let bytes = name_keyed();
    let prog = Program::parse(&bytes, 0).unwrap();
    assert_eq!(prog.format(), Format::Cff);
    assert_eq!(prog.glyph_count(), 5);
    // The default FontMatrix of the CFF note is 0.001.
    assert_eq!(prog.font_matrix(), [0.001, 0.0, 0.0, 0.001, 0.0, 0.0]);
}

#[test]
fn widths_come_from_nominal_plus_operand_or_the_default() {
    let bytes = name_keyed();
    let prog = Program::parse(&bytes, 0).unwrap();
    assert_eq!(prog.advance(GlyphId(1)), Some(700.0));
    assert_eq!(prog.advance(GlyphId(2)), Some(600.0));
    assert_eq!(prog.advance(GlyphId(3)), Some(250.0));
    assert_eq!(prog.advance(GlyphId(0)), Some(600.0));
}

#[test]
fn outlines_follow_the_type_2_operators() {
    let bytes = name_keyed();
    let prog = Program::parse(&bytes, 0).unwrap();
    let p = Point::new;
    assert_eq!(
        closed(prog.outline(GlyphId(1)).unwrap().segments),
        vec![
            Segment::MoveTo(p(100.0, 0.0)),
            Segment::LineTo(p(400.0, 0.0)),
            Segment::LineTo(p(400.0, 300.0)),
            Segment::LineTo(p(100.0, 300.0)),
            Segment::Close,
        ]
    );
    assert_eq!(
        closed(prog.outline(GlyphId(2)).unwrap().segments),
        vec![
            Segment::MoveTo(p(10.0, 20.0)),
            // (10,20) + (10,0) = c1 (20,20); + (20,10) = c2 (40,30); + (0,30) = (40,60)
            Segment::CubicTo(p(20.0, 20.0), p(40.0, 30.0), p(40.0, 60.0)),
            Segment::Close,
        ]
    );
}

#[test]
fn a_glyph_with_no_marks_is_an_empty_outline() {
    let bytes = name_keyed();
    let prog = Program::parse(&bytes, 0).unwrap();
    assert!(prog.outline(GlyphId(0)).unwrap().is_empty());
    assert!(prog.outline(GlyphId(3)).unwrap().is_empty());
}

#[test]
fn a_damaged_charstring_is_a_typed_error() {
    let bytes = name_keyed();
    let prog = Program::parse(&bytes, 0).unwrap();
    match prog.outline(GlyphId(4)) {
        Err(FontError::Outline { glyph, reason }) => {
            assert_eq!(glyph, GlyphId(4));
            assert_eq!(reason, "a damaged charstring");
        }
        other => panic!("expected an outline error, got {other:?}"),
    }
    assert_eq!(
        prog.outline(GlyphId(5)),
        Err(FontError::NoSuchGlyph(GlyphId(5)))
    );
}

#[test]
fn standard_encoding_and_the_charset_name_the_glyphs() {
    let bytes = name_keyed();
    let prog = Program::parse(&bytes, 0).unwrap();
    // Encoding offset 0 is StandardEncoding: code 65 is A, 66 is B, 32 is space.
    assert_eq!(prog.glyph_by_builtin_code(65), Some(GlyphId(1)));
    assert_eq!(prog.glyph_by_builtin_code(66), Some(GlyphId(2)));
    assert_eq!(prog.glyph_by_builtin_code(32), Some(GlyphId(3)));
    assert_eq!(prog.glyph_by_builtin_code(67), Some(GlyphId(4)));
    assert_eq!(prog.glyph_by_builtin_code(68), None);
    assert_eq!(prog.glyph_by_name("A"), Some(GlyphId(1)));
    assert_eq!(prog.glyph_by_name("space"), Some(GlyphId(3)));
    assert_eq!(prog.glyph_name(GlyphId(2)).as_deref(), Some("B"));
    assert_eq!(prog.glyph_by_name("Q"), None);
    // No cmap, no CIDs in a name-keyed program.
    assert_eq!(prog.glyph_by_unicode('A'), None);
    assert_eq!(prog.glyph_by_cid(34), None);
    // The Top DICT names no FontBBox, ItalicAngle or isFixedPitch: the
    // defaults of the CFF note (an absent box, angle 0, proportional).
    let m = prog.metrics();
    assert_eq!(
        (m.bbox, m.italic_angle, m.is_monospaced, m.is_italic),
        (None, Some(0.0), Some(false), Some(false))
    );
    assert_eq!(m.ascent, None);
}

#[test]
fn a_cid_keyed_program_maps_cids_through_its_charset() {
    let bytes = cid_keyed();
    let prog = Program::parse(&bytes, 0).unwrap();
    assert_eq!(prog.format(), Format::Cff);
    assert_eq!(prog.glyph_count(), 3);
    assert_eq!(prog.glyph_by_cid(500), Some(GlyphId(1)));
    assert_eq!(prog.glyph_by_cid(3), Some(GlyphId(2)));
    assert_eq!(prog.glyph_by_cid(4), None);
    // A CID-keyed font has no one-byte built-in encoding.
    assert_eq!(prog.glyph_by_builtin_code(65), None);
    let p = Point::new;
    assert_eq!(
        closed(prog.outline(GlyphId(2)).unwrap().segments),
        vec![
            Segment::MoveTo(p(1.0, 2.0)),
            Segment::LineTo(p(1.0, 22.0)),
            Segment::Close
        ]
    );
}

#[test]
fn the_font_matrix_in_the_top_dict_is_read() {
    let spec = CffSpec {
        glyphs: vec![CffGlyph {
            id: 0,
            code: Cs2::new().op("endchar"),
        }],
        default_width: DEFAULT,
        nominal_width: NOMINAL,
        matrix: Some("0.0005 0 0 0.0005 0 0"),
        cid_keyed: false,
        encoding: None,
    };
    let bytes = cff::build(&spec);
    let prog = Program::parse(&bytes, 0).unwrap();
    assert_eq!(prog.font_matrix(), [0.0005, 0.0, 0.0, 0.0005, 0.0, 0.0]);
}

#[test]
fn damaged_files_are_typed_errors_never_panics() {
    for bytes in [name_keyed(), cid_keyed()] {
        for cut in 0..bytes.len() {
            let _ = Program::parse(&bytes[..cut], 0);
        }
        for at in 0..bytes.len() {
            let mut damaged = bytes.clone();
            damaged[at] ^= 0xFF;
            if let Ok(prog) = Program::parse(&damaged, 0) {
                for g in 0..prog.glyph_count().min(64) {
                    let _ = prog.outline(GlyphId(g));
                    let _ = prog.advance(GlyphId(g));
                    let _ = prog.glyph_name(GlyphId(g));
                }
                let _ = prog.glyph_by_cid(500);
                let _ = prog.glyph_by_builtin_code(65);
            }
        }
    }
    assert_eq!(Program::parse(&[1, 0, 4, 1], 0).err(), Some(FontError::Cff));
}

#[test]
fn a_custom_encoding_is_the_whole_encoding_with_no_fallback_to_standard() {
    // Glyph 1 is A, glyph 2 is B, glyph 3 is space; the font's own Encoding
    // gives them codes 200, 201 and 202.
    let spec = CffSpec {
        glyphs: vec![
            CffGlyph {
                id: 0,
                code: Cs2::new().op("endchar"),
            },
            CffGlyph {
                id: 34,
                code: Cs2::new().op("endchar"),
            },
            CffGlyph {
                id: 35,
                code: Cs2::new().op("endchar"),
            },
            CffGlyph {
                id: 1,
                code: Cs2::new().op("endchar"),
            },
        ],
        default_width: DEFAULT,
        nominal_width: NOMINAL,
        matrix: None,
        cid_keyed: false,
        encoding: Some(vec![200, 201, 202]),
    };
    let bytes = cff::build(&spec);
    let prog = Program::parse(&bytes, 0).unwrap();
    assert_eq!(prog.glyph_by_builtin_code(200), Some(GlyphId(1)));
    assert_eq!(prog.glyph_by_builtin_code(201), Some(GlyphId(2)));
    assert_eq!(prog.glyph_by_builtin_code(202), Some(GlyphId(3)));
    // StandardEncoding would give 65 the glyph A; this font does not assign
    // code 65, so it has no glyph.
    assert_eq!(prog.glyph_by_builtin_code(65), None);
    assert_eq!(prog.glyph_by_builtin_code(32), None);
}
