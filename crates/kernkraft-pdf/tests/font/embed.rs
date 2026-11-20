// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Font embedding (`src/font/embed.rs`): ISO 32000-1:2008, 9.6.4 (subset
//! tags), 9.7 (composite fonts), 9.8 (descriptors), 9.9 (embedded programs)
//! and 9.10.3 (ToUnicode). The fonts are written table by table
//! (`embed_truetype`, `embed_opentype`), so the glyph numbers, advances and
//! boxes each test expects are the ones its font was built with.

use kernkraft_font::{Format, GlyphId, Program, Variations, AXIS_WEIGHT};
use kernkraft_pdf::build::ObjectBuilder;
use kernkraft_pdf::doc::Document;
use kernkraft_pdf::font::{FontEmbedder, FontKind, ProgramKind};
use kernkraft_pdf::syntax::{Dictionary, ObjectId, Value};
use kernkraft_pdf::PdfError;

use super::*;
use fontkit::cff::{self, Cs2};
use fontkit::sfnt::{self, Glyph, Move, SfntSpec, Variable};

fn embedder() -> FontEmbedder {
    FontEmbedder::new(embed_truetype(true), 0).unwrap()
}

/// The document of a page that shows `text` with `embedder`, and the font.
fn shown(embedder: &mut FontEmbedder, text: &str) -> (Document, ObjectId, Vec<u8>) {
    let codes = embedder.encode(text).unwrap();
    let (doc, font) = embedded_page(embedder, show_codes(&codes, 10.0, 1.0, 1.0));
    (doc, font, codes)
}

fn dictionary_of(doc: &Document, id: ObjectId) -> Dictionary {
    doc.required(id).unwrap().dictionary().unwrap().clone()
}

fn name_of<'a>(dictionary: &'a Dictionary, key: &str) -> &'a [u8] {
    match dictionary.get(key.as_bytes()).map(|o| o.value()) {
        Some(Value::Name(name)) => name.as_bytes(),
        other => panic!("{key} is {other:?}"),
    }
}

/// The descendant CIDFont's dictionary of the Type 0 font `font`.
fn descendant(doc: &Document, font: ObjectId) -> Dictionary {
    let type0 = dictionary_of(doc, font);
    let Some(Value::Array(items)) = type0.get(b"DescendantFonts").map(|o| o.value()) else {
        panic!("no DescendantFonts");
    };
    let Value::Reference(id) = items[0].value() else {
        panic!("the descendant is not a reference");
    };
    dictionary_of(doc, *id)
}

#[test]
fn text_is_two_bytes_a_character_numbered_in_the_order_the_glyphs_are_first_shown() {
    let mut e = embedder();
    // A is glyph 1 of the font and gets the number 1, B gets 2, and the
    // lower-case a is the same glyph as A.
    assert_eq!(e.encode("ABA").unwrap(), [0, 1, 0, 2, 0, 1]);
    assert_eq!(e.encode("a").unwrap(), [0, 1]);
    assert_eq!(e.encode("").unwrap(), Vec::<u8>::new());
    // A space is glyph 3 of the font and the third shown.
    assert_eq!(e.encode(" ").unwrap(), [0, 3]);
}

#[test]
fn the_written_font_reads_back_as_a_type_0_font_with_its_characters() {
    let mut e = embedder();
    let (doc, font_id, codes) = shown(&mut e, "ABA a");
    let font = load(&doc, font_id.number).unwrap();
    assert_eq!(font.kind(), FontKind::Type0);

    let chars: Vec<_> = font.chars(&codes).collect();
    assert_eq!(chars.len(), 5);
    let cids: Vec<u32> = chars.iter().map(|c| c.cid).collect();
    assert_eq!(cids, [1, 2, 1, 3, 1]);
    // The widths are the advances the font was built with.
    assert_eq!(widths(&font, &codes), [700.0, 450.0, 700.0, 250.0, 700.0]);
    // The text of a glyph is the first character it was shown for: the
    // lower-case a is shown as an A's glyph and reads back as A.
    assert_eq!(
        texts(&font, &codes),
        [
            Some("A".to_string()),
            Some("B".to_string()),
            Some("A".to_string()),
            Some(" ".to_string()),
            Some("A".to_string())
        ]
    );
}

#[test]
fn a_truetype_font_is_a_cidfonttype2_with_the_identity_map_and_a_font_file_2() {
    let mut e = embedder();
    let (doc, font_id, _) = shown(&mut e, "AB");
    let type0 = dictionary_of(&doc, font_id);
    assert_eq!(name_of(&type0, "Subtype"), b"Type0");
    assert_eq!(name_of(&type0, "Encoding"), b"Identity-H");
    let cid_font = descendant(&doc, font_id);
    assert_eq!(name_of(&cid_font, "Subtype"), b"CIDFontType2");
    assert_eq!(name_of(&cid_font, "CIDToGIDMap"), b"Identity");
    // 9.7.4.3: a run of widths from CID 0: .notdef 500, A 700, B 450.
    let Some(Value::Array(w)) = cid_font.get(b"W").map(|o| o.value()) else {
        panic!("no W");
    };
    assert_eq!(w[0].value(), &Value::Integer(0));
    let Value::Array(run) = w[1].value() else {
        panic!("the run is not an array");
    };
    let run: Vec<_> = run.iter().map(|o| o.value().clone()).collect();
    assert_eq!(run, [500, 700, 450].map(Value::Integer).to_vec());
    // Table 121: a CIDFontType2's name is the Type 0 font's.
    assert_eq!(
        type0.get(b"BaseFont").map(|o| o.value().clone()),
        cid_font.get(b"BaseFont").map(|o| o.value().clone())
    );

    let font = load(&doc, font_id.number).unwrap();
    let descriptor = font.descriptor().unwrap();
    assert_eq!(descriptor.font_file.unwrap().kind, ProgramKind::TrueType);
    // The program is the subset: .notdef, A and B.
    let program = font.embedded_program(&doc).unwrap().unwrap();
    assert_eq!(program.kind, ProgramKind::TrueType);
    let parsed = Program::parse(&program.data, 0).unwrap();
    assert_eq!(parsed.format(), Format::TrueType);
    assert_eq!(parsed.glyph_count(), 3);
    let rectangle = |w: f32, h: f32| {
        let p = kernkraft_font::Point::new;
        vec![
            kernkraft_font::Segment::MoveTo(p(0.0, 0.0)),
            kernkraft_font::Segment::LineTo(p(w, 0.0)),
            kernkraft_font::Segment::LineTo(p(w, h)),
            kernkraft_font::Segment::LineTo(p(0.0, h)),
            kernkraft_font::Segment::Close,
        ]
    };
    let outline = |g: u16| {
        parsed
            .outline(GlyphId(g))
            .unwrap()
            .segments
            .into_iter()
            .filter(
                |s| !matches!(s, kernkraft_font::Segment::LineTo(p) if p.x == 0.0 && p.y == 0.0),
            )
            .collect::<Vec<_>>()
    };
    assert_eq!(outline(1), rectangle(500.0, 700.0));
    assert_eq!(outline(2), rectangle(200.0, 200.0));
}

#[test]
fn the_descriptor_is_measured_from_the_font() {
    let mut e = embedder();
    let (doc, font_id, _) = shown(&mut e, "AB");
    let font = load(&doc, font_id.number).unwrap();
    let d = font.descriptor().unwrap();
    // Glyph codes are not the Adobe standard Latin set: symbolic.
    assert!(d.flags.symbolic());
    assert!(!d.flags.nonsymbolic());
    assert!(!d.flags.fixed_pitch());
    assert!(!d.flags.italic());
    // The box of A (0 0 500 700) and B (0 0 200 200).
    assert_eq!(d.font_bbox, Some([0.0, 0.0, 500.0, 700.0]));
    assert_eq!((d.ascent, d.descent), (800.0, -200.0));
    assert_eq!(d.cap_height, Some(700.0));
    assert_eq!(d.italic_angle, 0.0);
    // The stem is `l`, 80 units wide, which is in the font though not shown.
    assert_eq!(d.stem_v, f64::from(EMBED_STEM));
    assert_eq!(d.font_name, font.base_font().map(str::to_string));
}

#[test]
fn a_font_with_no_stem_glyph_has_a_stem_of_zero() {
    let mut e = FontEmbedder::new(embed_truetype(false), 0).unwrap();
    let (doc, font_id, _) = shown(&mut e, "A");
    let font = load(&doc, font_id.number).unwrap();
    assert_eq!(font.descriptor().unwrap().stem_v, 0.0);
}

#[test]
fn units_to_the_em_other_than_a_thousand_are_scaled_with_the_box_rounded_outwards() {
    // 2048 to the em: 1000/2048 glyph units a design unit.
    let mut spec = SfntSpec::new(vec![
        Glyph {
            name: ".notdef",
            advance: 1000,
            contours: vec![],
        },
        Glyph {
            name: "A",
            advance: 1024,
            // 1024 by 1434 units: 500 by 700.1953125 glyph units.
            contours: vec![vec![
                (0, 0, true),
                (1024, 0, true),
                (1024, 1434, true),
                (0, 1434, true),
            ]],
        },
    ]);
    spec.units_per_em = 2048;
    spec.ascender = 1600;
    spec.descender = -400;
    spec.unicode = vec![(0x41, 1)];
    let mut e = FontEmbedder::new(sfnt::build(&spec), 0).unwrap();
    assert_eq!(e.width("A").unwrap(), 500.0);
    let (doc, font_id, codes) = shown(&mut e, "A");
    let font = load(&doc, font_id.number).unwrap();
    assert_eq!(widths(&font, &codes), [500.0]);
    let d = font.descriptor().unwrap();
    // 1600 * 0.48828125 = 781.25 and -400 * 0.48828125 = -195.3125.
    assert_eq!((d.ascent, d.descent), (781.0, -195.0));
    // The upper edge 700.1953125 goes up, the corner at the origin stays.
    assert_eq!(d.font_bbox, Some([0.0, 0.0, 500.0, 701.0]));
}

#[test]
fn an_open_type_font_with_cff_outlines_is_a_cidfonttype0_with_a_font_file_3() {
    let mut e = FontEmbedder::new(embed_opentype(), 0).unwrap();
    let (doc, font_id, codes) = shown(&mut e, "BA");
    let type0 = dictionary_of(&doc, font_id);
    let cid_font = descendant(&doc, font_id);
    assert_eq!(name_of(&cid_font, "Subtype"), b"CIDFontType0");
    assert!(cid_font.get(b"CIDToGIDMap").is_none());
    // Table 121: a CIDFontType0's name has the CMap's after a hyphen.
    let base = String::from_utf8(name_of(&cid_font, "BaseFont").to_vec()).unwrap();
    assert_eq!(
        String::from_utf8(name_of(&type0, "BaseFont").to_vec()).unwrap(),
        format!("{base}-Identity-H")
    );

    let font = load(&doc, font_id.number).unwrap();
    assert_eq!(
        font.descriptor().unwrap().font_file.unwrap().kind,
        ProgramKind::OpenType
    );
    let program = font.embedded_program(&doc).unwrap().unwrap();
    assert_eq!(program.kind, ProgramKind::OpenType);
    let parsed = Program::parse(&program.data, 0).unwrap();
    assert_eq!(parsed.format(), Format::OpenTypeCff);
    assert_eq!(parsed.glyph_count(), 3);
    // B is shown first: B is 1 and A is 2.
    assert_eq!(codes, [0, 1, 0, 2]);
    assert_eq!(widths(&font, &codes), [450.0, 700.0]);
    assert_eq!(
        texts(&font, &codes),
        [Some("B".to_string()), Some("A".to_string())]
    );
    // The subset's CFF is CID-keyed: the CID is the glyph number.
    assert_eq!(parsed.glyph_by_cid(1), Some(GlyphId(1)));
    assert_eq!(parsed.glyph_by_cid(2), Some(GlyphId(2)));
}

#[test]
fn the_tag_and_the_objects_follow_the_font_and_the_glyphs_shown() {
    let write = |text: &str| {
        let mut e = embedder();
        let (doc, font_id, _) = shown(&mut e, text);
        let name =
            String::from_utf8(name_of(&dictionary_of(&doc, font_id), "BaseFont").to_vec()).unwrap();
        (doc, font_id, name)
    };
    let (first, first_id, name) = write("AB");
    let (second, second_id, again) = write("AB");
    assert_eq!(name, again);
    let (tag, postscript) = name.split_once('+').unwrap();
    assert_eq!(tag.len(), 6);
    assert!(tag.bytes().all(|b| b.is_ascii_uppercase()));
    assert_eq!(postscript, "TestSans-Regular");
    // The same font and glyphs give the same objects.
    assert_eq!(first_id, second_id);
    let program = |doc: &Document, id: ObjectId| {
        load(doc, id.number)
            .unwrap()
            .embedded_program(doc)
            .unwrap()
            .unwrap()
            .data
    };
    assert_eq!(program(&first, first_id), program(&second, second_id));
    // Other glyphs give another subset and another tag.
    let (_, _, other) = write("B");
    assert_ne!(other.split_once('+').unwrap().0, tag);
}

#[test]
fn characters_not_allowed_in_a_postscript_name_are_dropped() {
    let mut spec = SfntSpec::new(vec![
        Glyph {
            name: ".notdef",
            advance: 500,
            contours: vec![],
        },
        Glyph {
            name: "A",
            advance: 600,
            contours: vec![],
        },
    ]);
    spec.unicode = vec![(0x41, 1)];
    spec.postscript = "Test Sans (Bold)/1";
    let mut e = FontEmbedder::new(sfnt::build(&spec), 0).unwrap();
    let (doc, font_id, _) = shown(&mut e, "A");
    let base =
        String::from_utf8(name_of(&dictionary_of(&doc, font_id), "BaseFont").to_vec()).unwrap();
    assert_eq!(base.split_once('+').unwrap().1, "TestSansBold1");
}

#[test]
fn a_character_with_no_glyph_is_refused_and_adds_nothing() {
    let mut e = embedder();
    assert!(e.covers('A').unwrap());
    assert!(!e.covers('Z').unwrap());
    assert!(matches!(
        e.encode("AZ"),
        Err(PdfError::NoGlyphForCharacter { character: 'Z' })
    ));
    assert!(matches!(
        e.width("Z"),
        Err(PdfError::NoGlyphForCharacter { character: 'Z' })
    ));
    // A was not added by the failed call: B is the first glyph shown.
    assert_eq!(e.encode("B").unwrap(), [0, 1]);
}

#[test]
fn the_width_of_text_is_the_sum_of_its_advances_and_adds_nothing() {
    let mut e = embedder();
    assert_eq!(e.width("AB a").unwrap(), 700.0 + 450.0 + 250.0 + 700.0);
    assert_eq!(e.width("").unwrap(), 0.0);
    // Nothing was added to the subset: A is still the first glyph.
    assert_eq!(e.encode("A").unwrap(), [0, 1]);
}

#[test]
fn what_is_not_a_reducible_font_is_refused() {
    assert!(matches!(
        FontEmbedder::new(b"not a font".to_vec(), 0),
        Err(PdfError::Font(kernkraft_font::FontError::UnknownFormat))
    ));
    // A bare CFF program is a font this crate reads and cannot reduce.
    let bare = fontkit::cff::build(&fontkit::cff::CffSpec {
        glyphs: vec![fontkit::cff::CffGlyph {
            id: 0,
            code: fontkit::cff::Cs2::new().op("endchar"),
        }],
        default_width: 600,
        nominal_width: 500,
        matrix: None,
        cid_keyed: false,
        encoding: None,
    });
    assert!(matches!(
        FontEmbedder::new(bare, 0),
        Err(PdfError::Font(kernkraft_font::FontError::Subset(_)))
    ));
}

#[test]
fn several_fonts_can_be_written_into_one_set_of_changes() {
    let mut first = embedder();
    let mut second = FontEmbedder::new(embed_opentype(), 0).unwrap();
    first.encode("A").unwrap();
    second.encode("B").unwrap();
    let (base, _) = embedded_page(&first, show_codes(&[0, 1], 10.0, 1.0, 1.0));
    let mut builder = ObjectBuilder::new(&base);
    let one = first.write(&mut builder).unwrap();
    let two = second.write(&mut builder).unwrap();
    assert_ne!(one, two);
    let doc = base.apply(builder.finish()).unwrap();
    for id in [one, two] {
        assert_eq!(load(&doc, id.number).unwrap().kind(), FontKind::Type0);
    }
}

/// Everything an application does with a font it was handed: open it, show
/// text with it, write it.
fn use_font(bytes: Vec<u8>) -> Result<(), PdfError> {
    let mut e = FontEmbedder::new(bytes, 0)?;
    e.encode("AB ")?;
    let base = font_document(&[]).open();
    let mut builder = ObjectBuilder::new(&base);
    e.write(&mut builder)?;
    Ok(())
}

#[test]
fn damaged_fonts_are_typed_errors_never_panics() {
    for font in [embed_truetype(true), embed_opentype()] {
        assert!(use_font(font.clone()).is_ok());
        for len in (0..font.len()).step_by(11) {
            let _ = use_font(font[..len].to_vec());
        }
        for at in (0..font.len()).step_by(3) {
            let mut damaged = font.clone();
            damaged[at] ^= 0xFF;
            let _ = use_font(damaged);
        }
    }
}

/// A font with one `wght` axis, 400 (the default) to 900: glyph 1 is the
/// triangle (0,0) (100,0) (50,100) of advance 700, which at 900 has its
/// points moved by (0,0) (200,0) (100,50) and an advance of 1000.
fn variable_font() -> Vec<u8> {
    let mut spec = SfntSpec::new(vec![
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
    ]);
    spec.unicode = vec![(0x41, 1)];
    spec.variable = Some(Variable {
        tag: *b"wght",
        min: 400,
        default: 400,
        max: 900,
        instances: vec![],
        moves: vec![
            None,
            Some(Move {
                dx: vec![0, 200, 100],
                dy: vec![0, 0, 50],
                advance: 300,
            }),
        ],
    });
    sfnt::build(&spec)
}

fn weight(value: f32) -> Variations {
    Variations::new().with(AXIS_WEIGHT, value)
}

#[test]
fn a_variable_font_is_embedded_at_the_instance_chosen_with_its_widths_and_box() {
    // (weight, advance of A, box of A): halfway is 650, with half the deltas.
    for (value, advance, bbox) in [
        (400.0, 700.0, [0.0, 0.0, 100.0, 100.0]),
        (650.0, 850.0, [0.0, 0.0, 200.0, 125.0]),
        (900.0, 1000.0, [0.0, 0.0, 300.0, 150.0]),
    ] {
        let mut e = FontEmbedder::with_variations(variable_font(), 0, &weight(value)).unwrap();
        assert_eq!(e.width("A").unwrap(), advance, "width at {value}");
        let (doc, font_id, codes) = shown(&mut e, "A");
        let font = load(&doc, font_id.number).unwrap();
        assert_eq!(widths(&font, &codes), [advance], "W at {value}");
        let d = font.descriptor().unwrap();
        assert_eq!(d.font_bbox, Some(bbox), "box at {value}");
        // The program is a static subset at the instance.
        let program = font.embedded_program(&doc).unwrap().unwrap();
        assert_eq!(program.kind, ProgramKind::TrueType);
        let parsed = Program::parse(&program.data, 0).unwrap();
        assert!(parsed.axes().is_empty());
        assert_eq!(f64::from(parsed.advance(GlyphId(1)).unwrap()), advance);
        let outline = parsed.outline(GlyphId(1)).unwrap();
        assert_eq!(outline.control_box().map(|b| b.map(f64::from)), Some(bbox));
    }
}

#[test]
fn the_default_instance_is_what_new_embeds() {
    let program_of = |mut e: FontEmbedder| {
        let (doc, font_id, _) = shown(&mut e, "A");
        let font = load(&doc, font_id.number).unwrap();
        font.embedded_program(&doc).unwrap().unwrap().data
    };
    let plain = program_of(FontEmbedder::new(variable_font(), 0).unwrap());
    let named =
        program_of(FontEmbedder::with_variations(variable_font(), 0, &weight(400.0)).unwrap());
    assert_eq!(plain, named);
}

#[test]
fn different_instances_of_one_font_get_different_tags_and_names() {
    let names = |value: f32| {
        let mut e = FontEmbedder::with_variations(variable_font(), 0, &weight(value)).unwrap();
        let (doc, font_id, _) = shown(&mut e, "A");
        let font = load(&doc, font_id.number).unwrap();
        let base =
            String::from_utf8(name_of(&descendant(&doc, font_id), "BaseFont").to_vec()).unwrap();
        // The descriptor names the same font.
        assert_eq!(font.descriptor().unwrap().font_name, Some(base.clone()));
        base
    };
    let (regular, bold) = (names(400.0), names(900.0));
    assert_ne!(regular[..6], bold[..6]);
    // The default instance keeps the font's own name; any other is named by
    // Adobe TN 5902 (the test font has no named instance and no name ID 25).
    assert_eq!(&regular[6..], "+TestSans-Regular");
    assert_eq!(&bold[6..], "+TestSans_900wght");
}

#[test]
fn an_instance_the_font_does_not_have_is_refused_when_the_embedder_is_made() {
    use kernkraft_font::FontError;
    let make = |v: Variations| FontEmbedder::with_variations(variable_font(), 0, &v).map(drop);
    assert!(matches!(
        make(weight(901.0)),
        Err(PdfError::Font(FontError::AxisOutOfRange { .. }))
    ));
    assert!(matches!(
        make(Variations::new().with(*b"wdth", 100.0)),
        Err(PdfError::Font(FontError::UnknownAxis { .. }))
    ));
    assert!(matches!(
        make(weight(500.0).with(AXIS_WEIGHT, 600.0)),
        Err(PdfError::Font(FontError::DuplicateAxis { .. }))
    ));
    assert!(matches!(
        FontEmbedder::with_variations(embed_truetype(true), 0, &weight(400.0)),
        Err(PdfError::Font(FontError::NoVariationAxes))
    ));
}

#[test]
fn a_cff2_font_is_a_cidfonttype2_with_a_font_file_2() {
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
    spec.unicode = vec![(0x41, 1)];
    let base = sfnt::build(&spec);
    let bytes = sfnt::retable(
        &base,
        *b"OTTO",
        &[b"glyf", b"loca"],
        vec![(*b"CFF2", table)],
    );

    let mut e = FontEmbedder::new(bytes, 0).unwrap();
    let (doc, font_id, codes) = shown(&mut e, "A");
    let type0 = dictionary_of(&doc, font_id);
    let cid_font = descendant(&doc, font_id);
    // The outlines are redrawn as glyf: a TrueType CIDFont, not a CFF one.
    assert_eq!(name_of(&cid_font, "Subtype"), b"CIDFontType2");
    assert_eq!(name_of(&cid_font, "CIDToGIDMap"), b"Identity");
    assert_eq!(
        type0.get(b"BaseFont").map(|o| o.value().clone()),
        cid_font.get(b"BaseFont").map(|o| o.value().clone())
    );
    let font = load(&doc, font_id.number).unwrap();
    assert_eq!(
        font.descriptor().unwrap().font_file.unwrap().kind,
        ProgramKind::TrueType
    );
    let program = font.embedded_program(&doc).unwrap().unwrap();
    assert_eq!(program.kind, ProgramKind::TrueType);
    let parsed = Program::parse(&program.data, 0).unwrap();
    assert_eq!(parsed.format(), Format::TrueType);
    assert_eq!(widths(&font, &codes), [700.0]);
    assert_eq!(
        font.descriptor().unwrap().font_bbox,
        Some([0.0, 0.0, 100.0, 100.0])
    );
}
