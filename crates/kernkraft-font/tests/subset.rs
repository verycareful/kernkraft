// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Reducing a font to a glyph set: OpenType 1.9 (`glyf`, `loca`, `CFF `,
//! `hmtx`, `maxp`), ISO 32000-1:2008, 9.9.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::cff::{self, CffGlyph, CffSpec, Cs2};
use common::sfnt::{self, Glyph, SfntSpec};
use kernkraft_font::{
    subset, FontError, Format, GlyphId, GlyphSet, Outline, Point, Program, Segment,
};

/// notdef, a triangle (advance 700), a space (250), an arc (650) and a
/// composite of the arc moved by (10, 20) (800, patched in by `composite`).
fn font() -> Vec<u8> {
    let spec = SfntSpec::new(vec![
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
        Glyph {
            name: "moved",
            advance: 800,
            contours: sfnt::triangle(),
        },
    ]);
    sfnt::build(&spec)
}

const COMPONENT_GLYPH: u16 = 3;
const COMPOSITE_GLYPH: u16 = 4;
const COMPONENT_DX: i16 = 10;
const COMPONENT_DY: i16 = 20;

/// Glyph 4's description replaced by a composite of glyph 3 moved by
/// (10, 20) (OpenType `glyf`: numberOfContours -1, the box, then the flags
/// ARG_1_AND_2_ARE_WORDS | ARGS_ARE_XY_VALUES, the glyph index and the
/// offsets).
fn composite(mut bytes: Vec<u8>) -> Vec<u8> {
    let (glyf, len) = sfnt::table(&bytes, b"glyf").unwrap();
    let (loca, _) = sfnt::table(&bytes, b"loca").unwrap();
    // Short loca: offsets in halves.
    let at = |i: usize| {
        usize::from(u16::from_be_bytes([
            bytes[loca + 2 * i],
            bytes[loca + 2 * i + 1],
        ])) * 2
    };
    let (start, end) = (glyf + at(4), glyf + at(5));
    assert!(end <= glyf + len);
    let mut entry = Vec::new();
    entry.extend((-1i16).to_be_bytes());
    for v in [
        COMPONENT_DX,
        COMPONENT_DY,
        COMPONENT_DX + 100,
        COMPONENT_DY + 100,
    ] {
        entry.extend(v.to_be_bytes());
    }
    entry.extend(0x0003u16.to_be_bytes());
    entry.extend(COMPONENT_GLYPH.to_be_bytes());
    entry.extend(COMPONENT_DX.to_be_bytes());
    entry.extend(COMPONENT_DY.to_be_bytes());
    assert!(entry.len() <= end - start);
    entry.resize(end - start, 0);
    bytes[start..end].copy_from_slice(&entry);
    // The left side bearing is the box's xMin: the glyph's first point sits
    // at xMin - lsb, and drawing puts that point at the origin.
    let (hmtx, _) = sfnt::table(&bytes, b"hmtx").unwrap();
    let lsb = hmtx + 4 * usize::from(COMPOSITE_GLYPH) + 2;
    bytes[lsb..lsb + 2].copy_from_slice(&COMPONENT_DX.to_be_bytes());
    bytes
}

fn set_of(program: &Program, glyphs: &[u16]) -> GlyphSet {
    let mut set = GlyphSet::new(program.glyph_count());
    for g in glyphs {
        set.add(GlyphId(*g)).unwrap();
    }
    set
}

fn moved(outline: &Outline, dx: f32, dy: f32) -> Outline {
    let shift = |p: &Point| Point::new(p.x + dx, p.y + dy);
    Outline {
        segments: outline
            .segments
            .iter()
            .map(|s| match s {
                Segment::MoveTo(p) => Segment::MoveTo(shift(p)),
                Segment::LineTo(p) => Segment::LineTo(shift(p)),
                Segment::QuadTo(c, p) => Segment::QuadTo(shift(c), shift(p)),
                Segment::CubicTo(a, b, p) => Segment::CubicTo(shift(a), shift(b), shift(p)),
                Segment::Close => Segment::Close,
            })
            .collect(),
    }
}

#[test]
fn glyphs_are_numbered_in_the_order_they_are_added() {
    let bytes = font();
    let program = Program::parse(&bytes, 0).unwrap();
    let mut set = GlyphSet::new(program.glyph_count());
    assert_eq!(set.len(), 1);
    assert_eq!(set.get(GlyphId(0)), Some(0));
    assert_eq!(set.add(GlyphId(3)).unwrap(), 1);
    assert_eq!(set.add(GlyphId(1)).unwrap(), 2);
    // A glyph added twice keeps its number; .notdef is already there.
    assert_eq!(set.add(GlyphId(3)).unwrap(), 1);
    assert_eq!(set.add(GlyphId(0)).unwrap(), 0);
    assert_eq!(set.len(), 3);
    assert_eq!(set.get(GlyphId(2)), None);
    assert_eq!(
        set.glyphs().collect::<Vec<_>>(),
        vec![GlyphId(0), GlyphId(3), GlyphId(1)]
    );
    assert_eq!(set.add(GlyphId(5)), Err(FontError::NoSuchGlyph(GlyphId(5))));
}

#[test]
fn a_truetype_subset_keeps_each_glyph_outline_and_advance_under_its_new_number() {
    let bytes = font();
    let program = Program::parse(&bytes, 0).unwrap();
    let set = set_of(&program, &[3, 1]);
    let sub = subset(&bytes, 0, &set).unwrap();
    assert_eq!(sub.format, Format::TrueType);
    let small = Program::parse(&sub.data, 0).unwrap();
    assert_eq!(small.format(), Format::TrueType);
    assert_eq!(small.glyph_count(), 3);
    // New glyph 1 is the arc, new glyph 2 the triangle, 0 is .notdef.
    for (new, old) in [(0u16, 0u16), (1, 3), (2, 1)] {
        assert_eq!(
            small.outline(GlyphId(new)).unwrap(),
            program.outline(GlyphId(old)).unwrap(),
            "outline of glyph {old}"
        );
        assert_eq!(
            small.advance(GlyphId(new)),
            program.advance(GlyphId(old)),
            "advance of glyph {old}"
        );
    }
    assert_eq!(small.advance(GlyphId(1)), Some(650.0));
    assert_eq!(small.advance(GlyphId(2)), Some(700.0));
    assert!(sub.data.len() < bytes.len());
}

#[test]
fn the_components_of_a_composite_glyph_come_with_it() {
    let bytes = composite(font());
    let program = Program::parse(&bytes, 0).unwrap();
    // The source composite really is the arc moved by (10, 20).
    assert_eq!(
        program.outline(GlyphId(COMPOSITE_GLYPH)).unwrap(),
        moved(
            &program.outline(GlyphId(COMPONENT_GLYPH)).unwrap(),
            f32::from(COMPONENT_DX),
            f32::from(COMPONENT_DY)
        )
    );
    let set = set_of(&program, &[COMPOSITE_GLYPH]);
    let sub = subset(&bytes, 0, &set).unwrap();
    let small = Program::parse(&sub.data, 0).unwrap();
    // .notdef, the composite, and its component after the set's own.
    assert_eq!(small.glyph_count(), 3);
    assert_eq!(
        small.outline(GlyphId(1)).unwrap(),
        program.outline(GlyphId(COMPOSITE_GLYPH)).unwrap()
    );
    assert_eq!(
        small.outline(GlyphId(2)).unwrap(),
        program.outline(GlyphId(COMPONENT_GLYPH)).unwrap()
    );
    assert_eq!(small.advance(GlyphId(1)), Some(800.0));
}

#[test]
fn a_subset_of_one_font_in_a_collection_is_that_font() {
    let other = sfnt::build(&SfntSpec::new(vec![
        Glyph {
            name: ".notdef",
            advance: 100,
            contours: vec![],
        },
        Glyph {
            name: "arc",
            advance: 321,
            contours: sfnt::arc(),
        },
    ]));
    let collection = sfnt::collection(&[font(), other]);
    let program = Program::parse(&collection, 1).unwrap();
    let set = set_of(&program, &[1]);
    let sub = subset(&collection, 1, &set).unwrap();
    let small = Program::parse(&sub.data, 0).unwrap();
    assert_eq!(small.glyph_count(), 2);
    assert_eq!(small.advance(GlyphId(1)), Some(321.0));
    assert_eq!(
        small.outline(GlyphId(1)).unwrap(),
        program.outline(GlyphId(1)).unwrap()
    );
}

/// The TrueType font with its outlines replaced by a CFF table of four
/// glyphs: .notdef, a square, a space and a curve.
fn open_type_cff() -> Vec<u8> {
    let cff = cff::build(&CffSpec {
        glyphs: vec![
            CffGlyph {
                id: 0,
                code: Cs2::new().op("endchar"),
            },
            CffGlyph {
                id: 34,
                code: Cs2::new()
                    .with(&[200, 100, 0], "rmoveto")
                    .with(&[300], "hlineto")
                    .with(&[300], "vlineto")
                    .with(&[-300], "hlineto")
                    .op("endchar"),
            },
            CffGlyph {
                id: 1,
                code: Cs2::new().op("endchar"),
            },
            CffGlyph {
                id: 35,
                code: Cs2::new()
                    .with(&[10, 20], "rmoveto")
                    .with(&[10, 0, 20, 10, 0, 30], "rrcurveto")
                    .op("endchar"),
            },
        ],
        default_width: 600,
        nominal_width: 500,
        matrix: None,
        cid_keyed: false,
        encoding: None,
    });
    sfnt::open_type_with_cff(cff, &[500, 700, 250, 650], &[])
}

#[test]
fn an_open_type_cff_subset_keeps_each_glyph_outline_and_advance() {
    let bytes = open_type_cff();
    let program = Program::parse(&bytes, 0).unwrap();
    assert_eq!(program.format(), Format::OpenTypeCff);
    let set = set_of(&program, &[3, 1]);
    let sub = subset(&bytes, 0, &set).unwrap();
    assert_eq!(sub.format, Format::OpenTypeCff);
    let small = Program::parse(&sub.data, 0).unwrap();
    assert_eq!(small.format(), Format::OpenTypeCff);
    assert_eq!(small.glyph_count(), 3);
    for (new, old) in [(0u16, 0u16), (1, 3), (2, 1)] {
        assert_eq!(
            small.outline(GlyphId(new)).unwrap(),
            program.outline(GlyphId(old)).unwrap(),
            "outline of glyph {old}"
        );
        assert_eq!(
            small.advance(GlyphId(new)),
            program.advance(GlyphId(old)),
            "advance of glyph {old}"
        );
    }
    // The CFF is CID-keyed, with the new glyph number as the CID.
    assert_eq!(small.glyph_by_cid(1), Some(GlyphId(1)));
    assert_eq!(small.glyph_by_cid(2), Some(GlyphId(2)));
}

#[test]
fn what_cannot_be_reduced_is_a_typed_error() {
    let bytes = font();
    let program = Program::parse(&bytes, 0).unwrap();
    let set = set_of(&program, &[1]);

    // A set made for a font of another glyph count.
    let other = GlyphSet::new(7);
    assert!(matches!(
        subset(&bytes, 0, &other),
        Err(FontError::Subset(_))
    ));

    // Not an sfnt: a bare CFF program.
    let bare = cff::build(&CffSpec {
        glyphs: vec![CffGlyph {
            id: 0,
            code: Cs2::new().op("endchar"),
        }],
        default_width: 600,
        nominal_width: 500,
        matrix: None,
        cid_keyed: false,
        encoding: None,
    });
    assert!(matches!(
        subset(&bare, 0, &GlyphSet::new(1)),
        Err(FontError::Subset(_))
    ));

    // Not a font at all, and a collection index past the end.
    assert_eq!(subset(b"nothing", 0, &set), Err(FontError::UnknownFormat));
    let collection = sfnt::collection(std::slice::from_ref(&bytes));
    assert!(matches!(
        subset(&collection, 3, &set),
        Err(FontError::CollectionIndex { .. })
    ));
}

#[test]
fn damaged_fonts_are_typed_errors_never_panics() {
    let bytes = composite(font());
    let program = Program::parse(&bytes, 0).unwrap();
    let set = set_of(&program, &[1, 3, COMPOSITE_GLYPH]);
    for len in (0..bytes.len()).step_by(7) {
        let _ = subset(&bytes[..len], 0, &set);
    }
    for at in (0..bytes.len()).step_by(5) {
        let mut damaged = bytes.clone();
        damaged[at] ^= 0xFF;
        let _ = subset(&damaged, 0, &set);
    }
}
