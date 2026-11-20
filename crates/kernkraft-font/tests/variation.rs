// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Variable fonts: the axes of a font, a program read at an instance, and a
//! subset at an instance. OpenType 1.9 (`fvar`, `gvar`, `CFF2`) and ISO
//! 32000-1:2008, 9.9 (a PDF font program is a static font).
//!
//! The font is written table by table with one `wght` axis from 400 to 900
//! and the deltas below, so every expectation is the default outline plus
//! the fraction of the deltas the instance reaches: 650 is halfway, 900 all.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::cff::{self, Cs2};
use common::sfnt::{self, Glyph, Instance, Move, SfntSpec, Variable};
use kernkraft_font::{
    subset, Axis, FontError, Format, GlyphId, GlyphSet, Outline, Point, Program, Segment,
    Variations, AXIS_ITALIC, AXIS_SLANT, AXIS_WEIGHT,
};

const WGHT_MIN: i32 = 400;
const WGHT_DEFAULT: i32 = 400;
const WGHT_MAX: i32 = 900;
const HALFWAY: f32 = 650.0;
/// The triangle's points move by these at the axis maximum, and its advance
/// by `TRIANGLE_ADVANCE_DELTA`.
const TRIANGLE_DX: [i16; 3] = [0, 200, 100];
const TRIANGLE_DY: [i16; 3] = [0, 0, 50];
const TRIANGLE_ADVANCE_DELTA: i16 = 300;
const TRIANGLE: u16 = 1;
const BLANK: u16 = 2;
const ARC: u16 = 3;

fn glyphs() -> Vec<Glyph> {
    vec![
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
    ]
}

fn variable_font_on(tag: [u8; 4], min: i32, default: i32, max: i32, moves: bool) -> Vec<u8> {
    let mut spec = SfntSpec::new(glyphs());
    spec.unicode = vec![(0x41, TRIANGLE), (0x20, BLANK), (0x42, ARC)];
    spec.variable = Some(Variable {
        tag,
        min,
        default,
        max,
        instances: vec![],
        moves: vec![
            None,
            moves.then(|| Move {
                dx: TRIANGLE_DX.to_vec(),
                dy: TRIANGLE_DY.to_vec(),
                advance: TRIANGLE_ADVANCE_DELTA,
            }),
        ],
    });
    sfnt::build(&spec)
}

fn variable_font() -> Vec<u8> {
    variable_font_on(*b"wght", WGHT_MIN, WGHT_DEFAULT, WGHT_MAX, true)
}

fn static_font() -> Vec<u8> {
    let mut spec = SfntSpec::new(glyphs());
    spec.unicode = vec![(0x41, TRIANGLE)];
    sfnt::build(&spec)
}

fn weight(value: f32) -> Variations {
    Variations::new().with(AXIS_WEIGHT, value)
}

/// The fraction of the axis maximum's deltas an instance reaches.
fn reach(value: f32) -> f32 {
    (value - WGHT_DEFAULT as f32) / (WGHT_MAX - WGHT_DEFAULT) as f32
}

/// `outline` with its on-curve points (a triangle has no others) moved by
/// `reach` times the deltas.
fn displaced(outline: &Outline, reach: f32) -> Outline {
    let mut index = 0;
    let mut next = |p: &Point| {
        let at = Point::new(
            p.x + reach * f32::from(TRIANGLE_DX[index]),
            p.y + reach * f32::from(TRIANGLE_DY[index]),
        );
        index += 1;
        at
    };
    Outline {
        segments: outline
            .segments
            .iter()
            .map(|s| match s {
                Segment::MoveTo(p) => Segment::MoveTo(next(p)),
                Segment::LineTo(p) => Segment::LineTo(next(p)),
                Segment::Close => Segment::Close,
                other => panic!("not a triangle: {other:?}"),
            })
            .collect(),
    }
}

fn triangle_advance(reach: f32) -> f32 {
    700.0 + reach * f32::from(TRIANGLE_ADVANCE_DELTA)
}

fn set_of(program: &Program, glyphs: &[u16]) -> GlyphSet {
    let mut set = GlyphSet::new(program.glyph_count());
    for g in glyphs {
        set.add(GlyphId(*g)).unwrap();
    }
    set
}

#[test]
fn a_variable_font_lists_its_axes() {
    let bytes = variable_font();
    let program = Program::parse(&bytes, 0).unwrap();
    assert_eq!(
        program.axes(),
        vec![Axis {
            tag: AXIS_WEIGHT,
            min: 400.0,
            default: 400.0,
            max: 900.0
        }]
    );
}

#[test]
fn a_font_without_variation_tables_has_no_axes() {
    let bytes = static_font();
    assert!(Program::parse(&bytes, 0).unwrap().axes().is_empty());
}

#[test]
fn a_program_at_an_instance_gives_that_instances_outlines_and_advances() {
    let bytes = variable_font();
    let default = Program::parse(&bytes, 0).unwrap();
    let base = default.outline(GlyphId(TRIANGLE)).unwrap();
    assert_eq!(default.advance(GlyphId(TRIANGLE)), Some(700.0));
    for value in [HALFWAY, 900.0] {
        let at = Program::parse(&bytes, 0)
            .unwrap()
            .with_variations(&weight(value))
            .unwrap();
        let r = reach(value);
        assert_eq!(
            at.outline(GlyphId(TRIANGLE)).unwrap(),
            displaced(&base, r),
            "outline at {value}"
        );
        assert_eq!(
            at.advance(GlyphId(TRIANGLE)),
            Some(triangle_advance(r)),
            "advance at {value}"
        );
        // A glyph the axis does not move does not move.
        assert_eq!(at.advance(GlyphId(ARC)), Some(650.0));
        assert_eq!(
            at.outline(GlyphId(ARC)).unwrap(),
            default.outline(GlyphId(ARC)).unwrap()
        );
    }
}

#[test]
fn the_default_value_and_no_value_are_the_same_instance() {
    let bytes = variable_font();
    let none = Program::parse(&bytes, 0).unwrap();
    let named = Program::parse(&bytes, 0)
        .unwrap()
        .with_variations(&weight(WGHT_DEFAULT as f32))
        .unwrap();
    for g in 0..none.glyph_count() {
        assert_eq!(none.advance(GlyphId(g)), named.advance(GlyphId(g)));
        assert_eq!(
            none.outline(GlyphId(g)).unwrap(),
            named.outline(GlyphId(g)).unwrap()
        );
    }
}

#[test]
fn an_instance_the_font_does_not_have_is_a_typed_error() {
    let bytes = variable_font();
    let at = |v: Variations| Program::parse(&bytes, 0).unwrap().with_variations(&v);
    assert!(matches!(
        at(Variations::new().with(*b"wdth", 100.0)),
        Err(FontError::UnknownAxis { tag }) if tag == *b"wdth"
    ));
    for value in [399.0, 901.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert!(
            matches!(
                at(weight(value)),
                Err(FontError::AxisOutOfRange { tag }) if tag == AXIS_WEIGHT
            ),
            "{value}"
        );
    }
    assert!(matches!(
        at(weight(500.0).with(AXIS_WEIGHT, 600.0)),
        Err(FontError::DuplicateAxis { tag }) if tag == AXIS_WEIGHT
    ));
    let fixed = static_font();
    assert!(matches!(
        Program::parse(&fixed, 0)
            .unwrap()
            .with_variations(&weight(400.0)),
        Err(FontError::NoVariationAxes)
    ));
}

#[test]
fn the_weight_of_the_metrics_follows_the_instance() {
    let bytes = variable_font();
    let default = Program::parse(&bytes, 0).unwrap().metrics();
    assert_eq!(default.weight, Some(400));
    let bold = Program::parse(&bytes, 0)
        .unwrap()
        .with_variations(&weight(700.0))
        .unwrap()
        .metrics();
    assert_eq!(bold.weight, Some(700));
    assert_eq!(bold.is_italic, default.is_italic);
}

/// A font with the one axis `tag`, whose default instance is `default_italic`
/// with a `post` italic angle of `angle_tenths` tenths of a degree.
fn font_with_axis(
    tag: [u8; 4],
    (min, default, max): (i32, i32, i32),
    default_italic: bool,
    angle_tenths: i16,
) -> Vec<u8> {
    let mut spec = SfntSpec::new(glyphs());
    spec.variable = Some(Variable {
        tag,
        min,
        default,
        max,
        moves: vec![],
        instances: vec![],
    });
    spec.italic = default_italic;
    spec.italic_angle_tenths = angle_tenths;
    sfnt::build(&spec)
}

fn metrics_at(bytes: &[u8], v: &Variations) -> (Option<f32>, Option<bool>) {
    let m = Program::parse(bytes, 0)
        .unwrap()
        .with_variations(v)
        .unwrap()
        .metrics();
    (m.italic_angle, m.is_italic)
}

#[test]
fn the_slant_axis_sets_the_italic_angle_whatever_its_value() {
    // OpenType, registered axes: `slnt` matches `post` italicAngle at the
    // default, and for an instance its value can be used as the angle.
    let upright_default = font_with_axis(AXIS_SLANT, (0, 0, 10), false, 0);
    assert_eq!(
        metrics_at(&upright_default, &Variations::new()),
        (Some(0.0), Some(false))
    );
    assert_eq!(
        metrics_at(&upright_default, &Variations::new().with(AXIS_SLANT, 10.0)),
        (Some(10.0), Some(true))
    );
    // A font slanted by default, asked for upright: zero is a value, not an
    // absent one.
    let slanted_default = font_with_axis(AXIS_SLANT, (-12, -12, 0), true, -120);
    assert_eq!(
        metrics_at(&slanted_default, &Variations::new()),
        (Some(-12.0), Some(true))
    );
    assert_eq!(
        metrics_at(&slanted_default, &Variations::new().with(AXIS_SLANT, 0.0)),
        (Some(0.0), Some(false))
    );
}

#[test]
fn an_italic_axis_value_that_is_not_roman_is_italic() {
    // The `ital` axis is 0 for Roman and 1 for italic; the registry defines
    // nothing between, so anything but 0 is not Roman.
    let font = font_with_axis(AXIS_ITALIC, (0, 0, 1), false, 0);
    let italic = |v: f32| metrics_at(&font, &Variations::new().with(AXIS_ITALIC, v)).1;
    assert_eq!(metrics_at(&font, &Variations::new()).1, Some(false));
    assert_eq!(italic(0.0), Some(false));
    assert_eq!(italic(0.25), Some(true));
    assert_eq!(italic(1.0), Some(true));
}

/// A font named "Test Sans" with named instances Bold (700, no PostScript
/// name), Black (900, PostScript name `Test-Black-PS`), and, when `prefix` is
/// given, a `name` ID 25.
fn named_font(prefix: Option<&'static str>) -> Vec<u8> {
    const BOLD: u16 = 256;
    const BLACK: u16 = 257;
    const BLACK_POSTSCRIPT: u16 = 258;
    const PREFIX: u16 = 25;
    let mut spec = SfntSpec::new(glyphs());
    spec.extra_names = vec![
        (BOLD, "Bold"),
        (BLACK, "Black"),
        (BLACK_POSTSCRIPT, "Test-Black-PS"),
    ];
    if let Some(prefix) = prefix {
        spec.extra_names.push((PREFIX, prefix));
    }
    spec.variable = Some(Variable {
        tag: *b"wght",
        min: WGHT_MIN,
        default: WGHT_DEFAULT,
        max: WGHT_MAX,
        moves: vec![],
        instances: vec![
            Instance {
                name_id: BOLD,
                value: 700,
                postscript_name_id: None,
            },
            Instance {
                name_id: BLACK,
                value: 900,
                postscript_name_id: Some(BLACK_POSTSCRIPT),
            },
        ],
    });
    sfnt::build(&spec)
}

fn postscript_name_at(bytes: &[u8], v: &Variations) -> Option<String> {
    Program::parse(bytes, 0)
        .unwrap()
        .with_variations(v)
        .unwrap()
        .names()
        .postscript
}

#[test]
fn the_postscript_name_follows_the_instance_by_adobe_tn_5902() {
    let bytes = named_font(None);
    let name = |value: Option<f32>| {
        postscript_name_at(&bytes, &value.map_or_else(Variations::new, weight))
    };
    // The default instance keeps the font's own name (`name` ID 6).
    assert_eq!(name(None).as_deref(), Some("TestSans-Regular"));
    assert_eq!(name(Some(400.0)).as_deref(), Some("TestSans-Regular"));
    // A named instance: the prefix (the family without its space), a hyphen
    // and the subfamily name; or the PostScript name the font gives it.
    assert_eq!(name(Some(700.0)).as_deref(), Some("TestSans-Bold"));
    assert_eq!(name(Some(900.0)).as_deref(), Some("Test-Black-PS"));
    // Any other point: the axis value and tag after an underscore.
    assert_eq!(name(Some(650.0)).as_deref(), Some("TestSans_650wght"));
    assert_eq!(name(Some(512.5)).as_deref(), Some("TestSans_512.5wght"));
}

#[test]
fn a_variations_prefix_in_name_id_25_replaces_the_family_prefix() {
    let bytes = named_font(Some("TSVar"));
    assert_eq!(
        postscript_name_at(&bytes, &weight(650.0)).as_deref(),
        Some("TSVar_650wght")
    );
    assert_eq!(
        postscript_name_at(&bytes, &weight(700.0)).as_deref(),
        Some("TSVar-Bold")
    );
    // The default instance's name is not generated.
    assert_eq!(
        postscript_name_at(&bytes, &Variations::new()).as_deref(),
        Some("TestSans-Regular")
    );
}

#[test]
fn a_subset_at_an_instance_is_a_static_font_holding_that_instance() {
    let bytes = variable_font();
    let program = Program::parse(&bytes, 0).unwrap();
    let base = program.outline(GlyphId(TRIANGLE)).unwrap();
    for value in [HALFWAY, 900.0] {
        let set = set_of(&program, &[ARC, TRIANGLE]);
        let sub = subset(&bytes, 0, &set, &weight(value)).unwrap();
        assert_eq!(sub.format, Format::TrueType);
        assert!(sfnt::table(&sub.data, b"fvar").is_none(), "fvar at {value}");
        assert!(sfnt::table(&sub.data, b"gvar").is_none(), "gvar at {value}");
        let small = Program::parse(&sub.data, 0).unwrap();
        assert!(small.axes().is_empty());
        // New glyph 1 is the arc, 2 the triangle.
        let r = reach(value);
        assert_eq!(
            small.outline(GlyphId(2)).unwrap(),
            displaced(&base, r),
            "outline at {value}"
        );
        assert_eq!(small.advance(GlyphId(2)), Some(triangle_advance(r)));
        assert_eq!(small.advance(GlyphId(1)), Some(650.0));
        // The instance the program read is the instance the subset holds.
        let read = Program::parse(&bytes, 0)
            .unwrap()
            .with_variations(&weight(value))
            .unwrap();
        for (new, old) in [(1u16, ARC), (2, TRIANGLE)] {
            assert_eq!(
                small.outline(GlyphId(new)).unwrap(),
                read.outline(GlyphId(old)).unwrap()
            );
            assert_eq!(small.advance(GlyphId(new)), read.advance(GlyphId(old)));
        }
    }
}

#[test]
fn a_subset_at_the_default_is_the_default_instance_whether_or_not_it_is_named() {
    let bytes = variable_font();
    let program = Program::parse(&bytes, 0).unwrap();
    let set = set_of(&program, &[TRIANGLE]);
    let plain = subset(&bytes, 0, &set, &Variations::new()).unwrap();
    let named = subset(&bytes, 0, &set, &weight(WGHT_DEFAULT as f32)).unwrap();
    assert_eq!(plain, named);
    let small = Program::parse(&plain.data, 0).unwrap();
    assert_eq!(small.advance(GlyphId(1)), Some(700.0));
    assert_eq!(
        small.outline(GlyphId(1)).unwrap(),
        program.outline(GlyphId(TRIANGLE)).unwrap()
    );
}

#[test]
fn a_subset_refuses_an_instance_the_font_does_not_have() {
    let bytes = variable_font();
    let program = Program::parse(&bytes, 0).unwrap();
    let set = set_of(&program, &[TRIANGLE]);
    assert!(matches!(
        subset(&bytes, 0, &set, &weight(901.0)),
        Err(FontError::AxisOutOfRange { .. })
    ));
    assert!(matches!(
        subset(&bytes, 0, &set, &Variations::new().with(*b"opsz", 12.0)),
        Err(FontError::UnknownAxis { .. })
    ));
    let fixed = static_font();
    let fixed_set = set_of(&Program::parse(&fixed, 0).unwrap(), &[TRIANGLE]);
    assert!(matches!(
        subset(&fixed, 0, &fixed_set, &weight(400.0)),
        Err(FontError::NoVariationAxes)
    ));
}

/// An OpenType font whose outlines are a `CFF2` table: glyph 1 is the
/// triangle (0,0) (100,0) (50,100), advance 700.
fn cff2_font() -> Vec<u8> {
    let triangle = Cs2::new()
        .with(&[0, 0], "rmoveto")
        .with(&[100, 0], "rlineto")
        .with(&[-50, 100], "rlineto");
    let table = cff::build_cff2(&[Cs2::new(), triangle]);
    let mut spec = SfntSpec::new(vec![
        Glyph {
            name: ".notdef",
            advance: 500,
            contours: vec![],
        },
        Glyph {
            name: "tri",
            advance: 700,
            contours: vec![],
        },
    ]);
    spec.unicode = vec![(0x41, TRIANGLE)];
    let base = sfnt::build(&spec);
    sfnt::retable(
        &base,
        *b"OTTO",
        &[b"glyf", b"loca"],
        vec![(*b"CFF2", table)],
    )
}

#[test]
fn a_cff2_font_is_subset_as_a_truetype_font() {
    let bytes = cff2_font();
    let program = Program::parse(&bytes, 0).unwrap();
    // The source is an OpenType font with CFF outlines; the subset is not.
    assert_eq!(program.format(), Format::OpenTypeCff);
    let triangle = program.outline(GlyphId(TRIANGLE)).unwrap();
    assert_eq!(triangle.control_box(), Some([0.0, 0.0, 100.0, 100.0]));

    let set = set_of(&program, &[TRIANGLE]);
    let sub = subset(&bytes, 0, &set, &Variations::new()).unwrap();
    assert_eq!(sub.format, Format::TrueType);
    assert!(sfnt::table(&sub.data, b"glyf").is_some());
    assert!(sfnt::table(&sub.data, b"CFF2").is_none());
    let small = Program::parse(&sub.data, 0).unwrap();
    assert_eq!(small.format(), Format::TrueType);
    assert_eq!(small.glyph_count(), 2);
    assert_eq!(small.advance(GlyphId(1)), Some(700.0));
    assert_eq!(
        small.outline(GlyphId(1)).unwrap().control_box(),
        Some([0.0, 0.0, 100.0, 100.0])
    );
}

#[test]
fn a_cff2_font_without_axes_refuses_an_instance() {
    let bytes = cff2_font();
    let program = Program::parse(&bytes, 0).unwrap();
    let set = set_of(&program, &[TRIANGLE]);
    assert!(matches!(
        subset(&bytes, 0, &set, &weight(400.0)),
        Err(FontError::NoVariationAxes)
    ));
}

#[test]
fn a_cff_font_keeps_its_cff_outlines_in_the_subset() {
    // OpenType with a `CFF ` table is not redrawn: the format is read from
    // the subset and stays OpenType CFF.
    let cff_program = cff::build(&cff::CffSpec {
        glyphs: vec![
            cff::CffGlyph {
                id: 0,
                code: Cs2::new().op("endchar"),
            },
            cff::CffGlyph {
                id: 1,
                code: Cs2::new()
                    .with(&[0, 0], "rmoveto")
                    .with(&[100, 0], "rlineto")
                    .with(&[-50, 100], "rlineto")
                    .op("endchar"),
            },
        ],
        default_width: 600,
        nominal_width: 500,
        matrix: None,
        cid_keyed: false,
        encoding: None,
    });
    let bytes = sfnt::open_type_with_cff(cff_program, &[500, 700], &[(0x41, 1)]);
    let program = Program::parse(&bytes, 0).unwrap();
    let set = set_of(&program, &[1]);
    let sub = subset(&bytes, 0, &set, &Variations::new()).unwrap();
    assert_eq!(sub.format, Format::OpenTypeCff);
    assert!(sfnt::table(&sub.data, b"CFF ").is_some());
}
