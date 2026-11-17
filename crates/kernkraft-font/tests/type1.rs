// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Type 1 font programs: container, dictionaries, charstrings. Every
//! expectation is worked out by hand from the charstring semantics.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{Cs, Type1Spec};
use kernkraft_font::{FontError, Format, GlyphId, Outline, Point, Program, Segment};

fn p(x: f32, y: f32) -> Point {
    Point::new(x, y)
}

fn moveto(x: f32, y: f32) -> Segment {
    Segment::MoveTo(p(x, y))
}

fn lineto(x: f32, y: f32) -> Segment {
    Segment::LineTo(p(x, y))
}

/// A one-glyph font: `.notdef` first, then `g`.
fn one(code: Cs) -> Type1Spec {
    Type1Spec::new(vec![(".notdef", empty()), ("g", code)])
}

fn empty() -> Cs {
    Cs::new().with(&[0, 250], "hsbw").op("endchar")
}

fn outline_of(spec: &Type1Spec) -> Result<Outline, FontError> {
    let bytes = spec.flat();
    let prog = Program::parse(&bytes, 0).unwrap();
    let g = prog.glyph_by_name("g").unwrap();
    prog.outline(g)
}

fn segments(spec: &Type1Spec) -> Vec<Segment> {
    outline_of(spec).unwrap().segments
}

fn start() -> Cs {
    Cs::new()
}

#[test]
fn a_square_from_lines() {
    let spec = one(start()
        .with(&[50, 600], "hsbw")
        .with(&[100, 200], "rmoveto")
        .with(&[300], "hlineto")
        .with(&[300], "vlineto")
        .with(&[-300], "hlineto")
        .op("closepath")
        .op("endchar"));
    assert_eq!(
        segments(&spec),
        vec![
            // hsbw puts the pen at (50, 0); rmoveto 100 200 puts it at (150, 200).
            moveto(150.0, 200.0),
            lineto(450.0, 200.0),
            lineto(450.0, 500.0),
            lineto(150.0, 500.0),
            Segment::Close,
        ]
    );
    let bytes = spec.flat();
    let prog = Program::parse(&bytes, 0).unwrap();
    assert_eq!(prog.format(), Format::Type1);
    assert_eq!(prog.advance(prog.glyph_by_name("g").unwrap()), Some(600.0));
}

#[test]
fn the_three_curve_operators() {
    let spec = one(start()
        .with(&[0, 0], "hsbw")
        .with(&[0, 0], "rmoveto")
        // rrcurveto: (0,0) -> c1 (10,0), c2 (30,10), end (30,40)
        .with(&[10, 0, 20, 10, 0, 30], "rrcurveto")
        // vhcurveto 10 20 30 40: c1 (30,50), c2 (50,80), end (90,80)
        .with(&[10, 20, 30, 40], "vhcurveto")
        // hvcurveto 10 20 30 40: c1 (100,80), c2 (120,110), end (120,150)
        .with(&[10, 20, 30, 40], "hvcurveto")
        .op("closepath")
        .op("endchar"));
    let c = |a: (f32, f32), b: (f32, f32), e: (f32, f32)| {
        Segment::CubicTo(p(a.0, a.1), p(b.0, b.1), p(e.0, e.1))
    };
    assert_eq!(
        segments(&spec),
        vec![
            moveto(0.0, 0.0),
            c((10.0, 0.0), (30.0, 10.0), (30.0, 40.0)),
            c((30.0, 50.0), (50.0, 80.0), (90.0, 80.0)),
            c((100.0, 80.0), (120.0, 110.0), (120.0, 150.0)),
            Segment::Close,
        ]
    );
}

#[test]
fn moveto_variants_and_rlineto() {
    // hmoveto then vmoveto: the second replaces the first, so the glyph has
    // one contour starting at (10, 20).
    let spec = one(start()
        .with(&[0, 500], "hsbw")
        .with(&[10], "hmoveto")
        .with(&[20], "vmoveto")
        .with(&[5, 6], "rlineto")
        .with(&[7], "vlineto")
        .op("closepath")
        .op("endchar"));
    assert_eq!(
        segments(&spec),
        vec![
            moveto(10.0, 20.0),
            lineto(15.0, 26.0),
            lineto(15.0, 33.0),
            Segment::Close
        ]
    );
}

#[test]
fn sbw_sets_both_side_bearings() {
    let spec = one(start()
        .with(&[10, 20, 300, 0], "sbw")
        .with(&[0, 0], "rmoveto")
        .op("closepath")
        .op("endchar"));
    assert_eq!(segments(&spec), vec![moveto(10.0, 20.0), Segment::Close]);
    let bytes = spec.flat();
    let prog = Program::parse(&bytes, 0).unwrap();
    assert_eq!(prog.advance(prog.glyph_by_name("g").unwrap()), Some(300.0));
}

#[test]
fn a_subroutine_call_and_return() {
    let spec = one(start()
        .with(&[0, 500], "hsbw")
        .with(&[0, 0], "rmoveto")
        .with(&[0], "callsubr")
        .with(&[50], "vlineto")
        .op("closepath")
        .op("endchar"))
    .subrs(vec![start().with(&[100], "hlineto").op("return")]);
    assert_eq!(
        segments(&spec),
        vec![
            moveto(0.0, 0.0),
            lineto(100.0, 0.0),
            lineto(100.0, 50.0),
            Segment::Close
        ]
    );
}

#[test]
fn hint_replacement_goes_through_pop_and_callsubr() {
    // `1 1 3 callothersubr pop callsubr`: one operand (the subroutine
    // number 1), othersubr 3; `pop` brings the 1 back; `callsubr` runs it.
    let spec = one(start()
        .with(&[0, 500], "hsbw")
        .with(&[0, 0], "rmoveto")
        .with(&[1, 1, 3], "callothersubr")
        .op("pop")
        .op("callsubr")
        .op("closepath")
        .op("endchar"))
    .subrs(vec![
        start().op("return"),
        start().with(&[10], "hlineto").op("return"),
    ]);
    assert_eq!(
        segments(&spec),
        vec![moveto(0.0, 0.0), lineto(10.0, 0.0), Segment::Close]
    );
}

#[test]
fn division() {
    let spec = one(start()
        .with(&[0, 500], "hsbw")
        .with(&[0, 0], "rmoveto")
        .with(&[100, 4], "div")
        .op("hlineto")
        .op("closepath")
        .op("endchar"));
    assert_eq!(
        segments(&spec),
        vec![moveto(0.0, 0.0), lineto(25.0, 0.0), Segment::Close]
    );
}

#[test]
fn flex_becomes_two_curves() {
    // Points (absolute): reference (50,0), then (10,5) (20,8) (30,8) (40,8)
    // (50,5) (60,0); each rmoveto is relative to the point before it.
    let mut cs = start()
        .with(&[0, 1000], "hsbw")
        .with(&[0, 0], "rmoveto")
        .with(&[0, 1], "callothersubr");
    for d in [
        [50, 0],
        [-40, 5],
        [10, 3],
        [10, 0],
        [10, 0],
        [10, -3],
        [10, -5],
    ] {
        cs = cs.with(&d, "rmoveto").with(&[0, 2], "callothersubr");
    }
    cs = cs
        .with(&[50, 60, 0, 3, 0], "callothersubr")
        .op("pop")
        .op("pop")
        .op("setcurrentpoint")
        .with(&[7], "hlineto")
        .op("closepath")
        .op("endchar");
    let spec = one(cs);
    let c = |a: (f32, f32), b: (f32, f32), e: (f32, f32)| {
        Segment::CubicTo(p(a.0, a.1), p(b.0, b.1), p(e.0, e.1))
    };
    assert_eq!(
        segments(&spec),
        vec![
            moveto(0.0, 0.0),
            c((10.0, 5.0), (20.0, 8.0), (30.0, 8.0)),
            c((40.0, 8.0), (50.0, 5.0), (60.0, 0.0)),
            // setcurrentpoint put the pen at (60, 0).
            lineto(67.0, 0.0),
            Segment::Close,
        ]
    );
}

#[test]
fn seac_places_the_accent_by_the_accent_origin_offset() {
    let a = start()
        .with(&[10, 500], "hsbw")
        .with(&[0, 0], "rmoveto")
        .with(&[100], "hlineto")
        .with(&[100], "vlineto")
        .op("closepath")
        .op("endchar");
    let acute = start()
        .with(&[20, 300], "hsbw")
        .with(&[0, 0], "rmoveto")
        .with(&[50], "hlineto")
        .with(&[50], "vlineto")
        .op("closepath")
        .op("endchar");
    // asb 20, adx 60, ady 120, base 'A' (65), accent 'acute' (StandardEncoding 194).
    let aacute = start()
        .with(&[10, 500], "hsbw")
        .with(&[20, 60, 120, 65, 194], "seac");
    let spec = Type1Spec::new(vec![
        (".notdef", empty()),
        ("A", a),
        ("acute", acute),
        ("g", aacute),
    ]);
    // The accent moves by sbx + adx - asb = 10 + 60 - 20 = 50 across, 120 up.
    assert_eq!(
        segments(&spec),
        vec![
            moveto(10.0, 0.0),
            lineto(110.0, 0.0),
            lineto(110.0, 100.0),
            Segment::Close,
            moveto(70.0, 120.0),
            lineto(120.0, 120.0),
            lineto(120.0, 170.0),
            Segment::Close,
        ]
    );
    let bytes = spec.flat();
    let prog = Program::parse(&bytes, 0).unwrap();
    assert_eq!(prog.advance(prog.glyph_by_name("g").unwrap()), Some(500.0));
}

#[test]
fn a_contour_left_open_is_closed_at_the_next_moveto_and_at_endchar() {
    let spec = one(start()
        .with(&[0, 500], "hsbw")
        .with(&[0, 0], "rmoveto")
        .with(&[10], "hlineto")
        .with(&[5, 5], "rmoveto")
        .with(&[10], "hlineto")
        .op("endchar"));
    assert_eq!(
        segments(&spec),
        vec![
            moveto(0.0, 0.0),
            lineto(10.0, 0.0),
            Segment::Close,
            moveto(15.0, 5.0),
            lineto(25.0, 5.0),
            Segment::Close,
        ]
    );
}

#[test]
fn hints_leave_the_outline_alone() {
    let spec = one(start()
        .with(&[0, 500], "hsbw")
        .with(&[0, 20], "hstem")
        .with(&[0, 20], "vstem")
        .op("dotsection")
        .with(&[0, 1, 2, 3, 4, 5], "vstem3")
        .with(&[0, 1, 2, 3, 4, 5], "hstem3")
        .with(&[0, 0], "rmoveto")
        .op("closepath")
        .op("endchar"));
    assert_eq!(segments(&spec), vec![moveto(0.0, 0.0), Segment::Close]);
}

#[test]
fn number_encodings_at_their_boundaries() {
    // The advance is the second operand of hsbw.
    let advance = |wx: Cs| {
        let cs = Cs::new().n(0);
        let mut bytes = cs.bytes();
        bytes.extend(wx.bytes());
        bytes.push(13);
        bytes.push(14);
        let spec = Type1Spec::new(vec![(".notdef", empty()), ("g", Cs(bytes))]);
        let font = spec.flat();
        let prog = Program::parse(&font, 0).unwrap();
        prog.advance(prog.glyph_by_name("g").unwrap()).unwrap()
    };
    // Written out by hand from the ranges of the charstring chapter.
    let raw = |bytes: &[u8]| Cs(bytes.to_vec());
    assert_eq!(advance(raw(&[32])), -107.0);
    assert_eq!(advance(raw(&[246])), 107.0);
    assert_eq!(advance(raw(&[247, 0])), 108.0);
    assert_eq!(advance(raw(&[250, 255])), 1131.0);
    assert_eq!(advance(raw(&[251, 0])), -108.0);
    assert_eq!(advance(raw(&[254, 255])), -1131.0);
    assert_eq!(advance(raw(&[255, 0, 0, 1, 0])), 256.0);
    assert_eq!(advance(raw(&[255, 0xFF, 0xFF, 0xFF, 0xFF])), -1.0);
    // And the helper's own encoder agrees with the table.
    assert_eq!(Cs::new().n(1131).0, vec![250, 255]);
    assert_eq!(Cs::new().n(-1131).0, vec![254, 255]);
}

#[test]
fn lookup_by_name_code_and_number() {
    let spec = Type1Spec::new(vec![
        (".notdef", empty()),
        ("A", empty()),
        ("acute", empty()),
    ]);
    let bytes = spec.flat();
    let prog = Program::parse(&bytes, 0).unwrap();
    assert_eq!(prog.glyph_count(), 3);
    assert_eq!(prog.glyph_by_name("acute"), Some(GlyphId(2)));
    assert_eq!(prog.glyph_name(GlyphId(1)).as_deref(), Some("A"));
    assert_eq!(prog.glyph_by_name("zzz"), None);
    // StandardEncoding: 65 is A, 194 (octal 302) is acute, 66 is B which the font lacks.
    assert_eq!(prog.glyph_by_builtin_code(65), Some(GlyphId(1)));
    assert_eq!(prog.glyph_by_builtin_code(194), Some(GlyphId(2)));
    assert_eq!(prog.glyph_by_builtin_code(66), None);
    assert_eq!(prog.glyph_by_unicode('A'), None);
}

#[test]
fn a_custom_encoding_array() {
    let mut spec = Type1Spec::new(vec![
        (".notdef", empty()),
        ("A", empty()),
        ("acute", empty()),
    ]);
    spec.encoding = Some(vec![(65, "A".into()), (1, "acute".into())]);
    let bytes = spec.flat();
    let prog = Program::parse(&bytes, 0).unwrap();
    assert_eq!(prog.glyph_by_builtin_code(65), Some(GlyphId(1)));
    assert_eq!(prog.glyph_by_builtin_code(1), Some(GlyphId(2)));
    // 194 is acute in StandardEncoding, but this font names its own.
    assert_eq!(prog.glyph_by_builtin_code(194), None);
}

#[test]
fn dictionaries_names_and_matrix() {
    let mut spec = Type1Spec::new(vec![(".notdef", empty())]);
    spec.font_matrix = Some("[0.0005 0 0 0.0005 0 0]".into());
    let bytes = spec.flat();
    let prog = Program::parse(&bytes, 0).unwrap();
    assert_eq!(prog.font_matrix(), [0.0005, 0.0, 0.0, 0.0005, 0.0, 0.0]);
    let n = prog.names();
    assert_eq!(n.postscript.as_deref(), Some("TestFont"));
    assert_eq!(n.full.as_deref(), Some("Test Font"));
    assert_eq!(n.family.as_deref(), Some("Test"));
    let m = prog.metrics();
    assert_eq!(m.italic_angle, Some(-12.0));
    assert_eq!(m.is_monospaced, Some(true));
    assert_eq!(m.bbox, Some([-10.0, -20.0, 1000.0, 900.0]));
}

#[test]
fn the_default_matrix_is_one_thousand_units() {
    let spec = Type1Spec::new(vec![(".notdef", empty())]);
    let mut parts = spec.cleartext();
    let at = parts.windows(11).position(|w| w == b"/FontMatrix").unwrap();
    let end = at + parts[at..].iter().position(|&b| b == b'\n').unwrap() + 1;
    parts.drain(at..end);
    parts.extend_from_slice(&common::encrypt(&spec.private(), 55665, 4));
    let prog = Program::parse(&parts, 0).unwrap();
    assert_eq!(prog.font_matrix(), [0.001, 0.0, 0.0, 0.001, 0.0, 0.0]);
}

#[test]
fn hex_binary_and_pfb_give_the_same_font() {
    let spec = one(start()
        .with(&[50, 600], "hsbw")
        .with(&[100, 200], "rmoveto")
        .with(&[300], "hlineto")
        .op("closepath")
        .op("endchar"));
    let expect = segments(&spec);
    for bytes in [spec.hex(), spec.pfb()] {
        let prog = Program::parse(&bytes, 0).unwrap();
        let g = prog.glyph_by_name("g").unwrap();
        assert_eq!(prog.outline(g).unwrap().segments, expect);
    }
}

#[test]
fn len_iv_zero_and_unencrypted() {
    for len_iv in [0, 1, 7, -1] {
        let mut spec = one(start()
            .with(&[0, 500], "hsbw")
            .with(&[3, 4], "rmoveto")
            .op("closepath")
            .op("endchar"))
        .subrs(vec![start().op("return")]);
        spec.len_iv = len_iv;
        assert_eq!(
            segments(&spec),
            vec![moveto(3.0, 4.0), Segment::Close],
            "lenIV {len_iv}"
        );
    }
}

#[test]
fn the_dash_bar_spelling_of_rd() {
    let mut spec = one(start()
        .with(&[0, 500], "hsbw")
        .with(&[3, 4], "rmoveto")
        .op("endchar"));
    spec.rd = "-|";
    assert_eq!(segments(&spec), vec![moveto(3.0, 4.0), Segment::Close]);
}

#[test]
fn an_empty_glyph_has_an_empty_outline() {
    let spec = one(empty());
    assert!(outline_of(&spec).unwrap().is_empty());
}

fn reason(spec: &Type1Spec) -> &'static str {
    match outline_of(spec) {
        Err(FontError::Outline { reason, .. }) => reason,
        other => panic!("expected an outline error, got {other:?}"),
    }
}

#[test]
fn damaged_charstrings_are_typed_errors() {
    let no_move = one(start()
        .with(&[0, 500], "hsbw")
        .with(&[5], "hlineto")
        .op("endchar"));
    assert_eq!(reason(&no_move), "a line before any moveto");

    let no_end = one(start().with(&[0, 500], "hsbw").with(&[0, 0], "rmoveto"));
    assert_eq!(reason(&no_end), "the charstring has no endchar");

    let no_hsbw = one(start().op("endchar"));
    assert_eq!(reason(&no_hsbw), "the charstring has no hsbw");

    let few = one(start()
        .with(&[0, 500], "hsbw")
        .with(&[1], "rlineto")
        .op("endchar"));
    assert_eq!(reason(&few), "a command has fewer operands than it takes");

    let zero = one(start()
        .with(&[0, 500], "hsbw")
        .with(&[1, 0], "div")
        .op("endchar"));
    assert_eq!(reason(&zero), "a division by zero");

    let mut bad = start().with(&[0, 500], "hsbw");
    bad.0.push(15);
    let undefined = one(bad.op("endchar"));
    assert_eq!(
        reason(&undefined),
        "a charstring command that is not defined"
    );

    let missing = one(start()
        .with(&[0, 500], "hsbw")
        .with(&[3], "callsubr")
        .op("endchar"));
    assert_eq!(reason(&missing), "a subroutine that does not exist");

    let pop = one(start().with(&[0, 500], "hsbw").op("pop").op("endchar"));
    assert_eq!(reason(&pop), "pop with nothing on the PostScript stack");

    let cut = one(Cs(vec![247]));
    assert_eq!(reason(&cut), "a number is cut short");
}

#[test]
fn the_operand_stack_holds_24_numbers() {
    let mut cs = start();
    for _ in 0..24 {
        cs = cs.n(1);
    }
    let fits = one(cs.clone().with(&[], "hsbw").op("endchar"));
    // hsbw takes the top two of 24 and clears the rest.
    assert!(outline_of(&fits).is_ok());
    let over = one(cs.n(1).with(&[], "hsbw").op("endchar"));
    assert_eq!(reason(&over), "the operand stack overflowed");
}

#[test]
fn subroutines_nest_ten_deep_and_no_more() {
    // Subroutine k calls subroutine k + 1; the last returns.
    let chain = |len: usize| {
        let mut subrs: Vec<Cs> = (0..len)
            .map(|k| start().with(&[k as i32 + 1], "callsubr").op("return"))
            .collect();
        subrs[len - 1] = start().op("return");
        subrs
    };
    let glyph = || {
        start()
            .with(&[0, 500], "hsbw")
            .with(&[0], "callsubr")
            .op("endchar")
    };
    // The glyph's call is depth 0, so a chain of ten subroutines is ten nested calls.
    assert!(outline_of(&one(glyph()).subrs(chain(10))).is_ok());
    assert_eq!(
        reason(&one(glyph()).subrs(chain(11))),
        "subroutines nest too deep"
    );
}

#[test]
fn a_subroutine_that_calls_itself_stops() {
    let spec = one(start()
        .with(&[0, 500], "hsbw")
        .with(&[0], "callsubr")
        .op("endchar"))
    .subrs(vec![start().with(&[0], "callsubr").op("return")]);
    assert_eq!(reason(&spec), "subroutines nest too deep");
}

#[test]
fn exponential_fan_out_is_cut_by_the_operation_cap() {
    // Each subroutine calls the next ten times: 10^9 commands without a cap.
    let mut subrs = Vec::new();
    for k in 0..9 {
        let mut c = start();
        for _ in 0..10 {
            c = c.with(&[k + 1], "callsubr");
        }
        subrs.push(c.op("return"));
    }
    subrs.push(start().op("return"));
    let spec = one(start()
        .with(&[0, 500], "hsbw")
        .with(&[0], "callsubr")
        .op("endchar"))
    .subrs(subrs);
    assert_eq!(reason(&spec), "the glyph runs too many commands");
}

#[test]
fn seac_cannot_name_a_missing_or_composite_character() {
    let missing = one(start()
        .with(&[0, 500], "hsbw")
        .with(&[0, 0, 0, 65, 194], "seac"));
    assert_eq!(reason(&missing), "a seac character the font does not hold");

    let inner = start()
        .with(&[0, 500], "hsbw")
        .with(&[0, 0, 0, 65, 65], "seac");
    let spec = Type1Spec::new(vec![
        (".notdef", empty()),
        ("A", inner),
        (
            "g",
            start()
                .with(&[0, 500], "hsbw")
                .with(&[0, 0, 0, 65, 65], "seac"),
        ),
    ]);
    assert_eq!(reason(&spec), "a composite glyph inside a composite glyph");

    let outside = one(start()
        .with(&[0, 500], "hsbw")
        .with(&[0, 0, 0, 300, 65], "seac"));
    assert_eq!(reason(&outside), "a seac character code outside 0 to 255");
}

#[test]
fn flex_errors() {
    let short = one(start()
        .with(&[0, 500], "hsbw")
        .with(&[0, 0], "rmoveto")
        .with(&[0, 1], "callothersubr")
        .with(&[50, 0, 0, 3, 0], "callothersubr"));
    assert_eq!(reason(&short), "a flex without seven points");
    let stray = one(start()
        .with(&[0, 500], "hsbw")
        .with(&[0, 2], "callothersubr"));
    assert_eq!(reason(&stray), "a flex point outside a flex");
}

#[test]
fn a_glyph_number_past_the_end() {
    let spec = one(empty());
    let bytes = spec.flat();
    let prog = Program::parse(&bytes, 0).unwrap();
    assert_eq!(
        prog.outline(GlyphId(2)),
        Err(FontError::NoSuchGlyph(GlyphId(2)))
    );
    assert_eq!(prog.advance(GlyphId(2)), None);
}

#[test]
fn damaged_files_are_typed_errors_never_panics() {
    let spec = one(start()
        .with(&[0, 500], "hsbw")
        .with(&[3, 4], "rmoveto")
        .op("endchar"));
    let font = spec.flat();
    // Every truncation either parses or is a typed error.
    for cut in 0..font.len() {
        let _ = Program::parse(&font[..cut], 0);
    }
    // Every single-byte change likewise, tried over the whole file.
    for at in (0..font.len()).step_by(3) {
        let mut damaged = font.clone();
        damaged[at] ^= 0x5A;
        if let Ok(prog) = Program::parse(&damaged, 0) {
            for g in 0..prog.glyph_count() {
                let _ = prog.outline(GlyphId(g));
            }
        }
    }
    assert!(matches!(
        Program::parse(b"%!PS-AdobeFont-1.0\n", 0),
        Err(FontError::Type1(_))
    ));
    assert!(matches!(
        Program::parse(b"\x00\x00", 0),
        Err(FontError::UnknownFormat)
    ));
}
