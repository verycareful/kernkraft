// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The profile DeviceCMYK is converted through: ISO 32000-1:2008, 14.11.5
//! (output intents) and 8.6.5.6 (default colour spaces), with the profile
//! format of ICC.1:2010.
//!
//! The profiles are written here, byte by byte: a CMYK profile whose
//! lightness follows black alone (so what it gives follows from its table,
//! not from a run), an sRGB profile written by `moxcms`, and a file that
//! says CMYK and is not a profile. The bundled profile is checked against
//! `moxcms` converting its bytes directly.

use super::*;
use kernkraft_pdf::colour::{CmykProfile, CmykSource, BUNDLED_PROFILE};
use kernkraft_pdf::doc::ObjectBuilder;
use moxcms::{ColorProfile, Layout, RenderingIntent, TransformOptions};
use sha2::{Digest, Sha256};

/// What an 8-bit pipeline can tell apart: the profile transform is done
/// in 14-bit and fixed-point arithmetic.
const TOLERANCE: f64 = 0.01;
/// The size and SHA-256 of FOGRA39L_coated.icc of the colorprofiles
/// package as TeX Live installs it, the file the library bundles.
const BUNDLED_LENGTH: usize = 121_368;
const BUNDLED_SHA256: &str = "b35713ef7eff09349d4c3249e5f377736d06d8a2671c54712971a3546bf17c57";

// Profiles written by hand (the first is `pdf_fixture::grey_by_black_profile`).

/// Says CMYK in its header and is nothing else.
fn broken_cmyk_profile() -> Vec<u8> {
    let mut bytes = vec![0u8; 256];
    bytes[16..20].copy_from_slice(b"CMYK");
    bytes
}

fn srgb_profile() -> Vec<u8> {
    ColorProfile::new_srgb().encode().unwrap()
}

/// The sRGB lightness of the neutral `L*`: `Y` of CIE 1976 (CIE 15, 8.2.1),
/// encoded by the sRGB curve.
fn grey_of_lightness(lightness: f64) -> f64 {
    let y = if lightness > 8.0 {
        ((lightness + 16.0) / 116.0).powi(3)
    } else {
        lightness / 903.3
    };
    srgb_encode(y)
}

/// The grey `grey_by_black_profile` gives with `k` black ink.
fn grey_by_black(k: f64) -> f64 {
    grey_of_lightness(100.0 * (1.0 - k))
}

// Documents with output intents.

/// A document whose catalog names `intents` (the profile streams are
/// objects 70 and up, the intent dictionaries 61 and up).
fn with_intents(profiles: &[&[u8]]) -> Document {
    let mut objects = pages_document(1);
    let mut intents = Vec::new();
    for (at, profile) in profiles.iter().enumerate() {
        let intent = 61 + at as u32;
        let stream_number = 70 + at as u32;
        intents.push(format!("{intent} 0 R"));
        objects.push((
            intent,
            format!(
                "<< /Type /OutputIntent /S /GTS_PDFX /OutputConditionIdentifier (Test) \
                 /DestOutputProfile {stream_number} 0 R >>"
            )
            .into_bytes(),
        ));
        objects.push((stream_number, stream(" /N 4", profile)));
    }
    objects[0] = (
        1,
        format!(
            "<< /Type /Catalog /Pages 2 0 R /OutputIntents [{}] >>",
            intents.join(" ")
        )
        .into_bytes(),
    );
    table_file(&borrowed(&objects), "/Root 1 0 R /Info 3 0 R").open()
}

fn cmyk(document: &Document, colour: [f64; 4]) -> Rgb {
    document.device_cmyk().unwrap().to_rgb(&colour).unwrap()
}

/// `colour` through `profile` by `moxcms` alone: relative colorimetric to
/// sRGB (8.6.5.8).
fn through(profile: &[u8], colour: [f64; 4]) -> [f64; 3] {
    let source = ColorProfile::new_from_slice(profile).unwrap();
    let options = TransformOptions {
        rendering_intent: RenderingIntent::RelativeColorimetric,
        ..TransformOptions::default()
    };
    let transform = source
        .create_transform_f32(
            Layout::Rgba,
            &ColorProfile::new_srgb(),
            Layout::Rgb,
            options,
        )
        .unwrap();
    let source = colour.map(|value| value as f32);
    let mut out = [0f32; 3];
    transform.transform(&source, &mut out).unwrap();
    out.map(f64::from)
}

// The bundled profile.

/// The file is the one of the colorprofiles package, unmodified: its
/// licence allows the file to be redistributed and altered, but an altered
/// one must say so.
#[test]
fn the_bundled_profile_is_the_file_of_the_colorprofiles_package() {
    assert_eq!(BUNDLED_PROFILE.len(), BUNDLED_LENGTH);
    let digest: String = Sha256::digest(BUNDLED_PROFILE)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(digest, BUNDLED_SHA256);
}

#[test]
fn the_bundled_profile_can_be_used() {
    let profile = CmykProfile::bundled().unwrap();
    assert_eq!(profile.source(), CmykSource::Bundled);
}

/// A file with no output intent and no profile from the application
/// converts through the bundled profile, as `moxcms` converts its bytes.
#[test]
fn without_an_intent_or_a_profile_the_bundled_one_is_used() {
    let document = document(&[]);
    assert_eq!(
        document.cmyk_profile().unwrap().source(),
        CmykSource::Bundled
    );
    for colour in [
        [0.0, 0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
        [0.25, 0.5, 0.0, 0.5],
        [0.8, 0.0, 0.0, 0.8],
    ] {
        assert_rgb(
            cmyk(&document, colour),
            through(BUNDLED_PROFILE, colour),
            1e-6,
        );
    }
    assert!(of_kind(&document, &DiagnosticKind::IccProfileUnusable).is_empty());
}

/// What a printed CMYK looks like on a screen, by the inks' colours: paper
/// is white, black ink is dark, and each ink is its own hue. Cyan has the
/// least red, magenta the least green and yellow the least blue.
#[test]
fn the_bundled_profile_gives_inks_their_colours() {
    let document = document(&[]);
    let paper = cmyk(&document, [0.0; 4]);
    assert_rgb(paper, [1.0; 3], 0.02);
    let cyan = cmyk(&document, [1.0, 0.0, 0.0, 0.0]);
    assert!(cyan.r < cyan.g && cyan.r < cyan.b, "{cyan:?}");
    let magenta = cmyk(&document, [0.0, 1.0, 0.0, 0.0]);
    assert!(
        magenta.g < magenta.r && magenta.g < magenta.b,
        "{magenta:?}"
    );
    let yellow = cmyk(&document, [0.0, 0.0, 1.0, 0.0]);
    assert!(yellow.b < yellow.r && yellow.b < yellow.g, "{yellow:?}");
    let black = cmyk(&document, [0.0, 0.0, 0.0, 1.0]);
    assert!(
        black.r < 0.25 && black.g < 0.25 && black.b < 0.25,
        "{black:?}"
    );
}

/// Two other renderers' sRGB for the full inks (as 8-bit levels), which the
/// FOGRA39 data comes near: cyan (0, 173, 239), magenta (236, 0, 140),
/// yellow (255, 242, 0) and black (35, 31, 32). They are not the same
/// conversion, so the tolerance is a few percent of the range.
#[test]
fn the_bundled_profile_is_near_other_renderers() {
    const NEAR: f64 = 16.0 / 255.0;
    let document = document(&[]);
    for (colour, expected) in [
        ([1.0, 0.0, 0.0, 0.0], [0.0, 173.0, 239.0]),
        ([0.0, 1.0, 0.0, 0.0], [236.0, 0.0, 140.0]),
        ([0.0, 0.0, 1.0, 0.0], [255.0, 242.0, 0.0]),
        ([0.0, 0.0, 0.0, 1.0], [35.0, 31.0, 32.0]),
    ] {
        assert_rgb(
            cmyk(&document, colour),
            expected.map(|level| level / 255.0),
            NEAR,
        );
    }
}

// The profile of a file's output intent (14.11.5).

#[test]
fn the_profile_of_an_output_intent_is_used() {
    let document = with_intents(&[&grey_by_black_profile()]);
    assert_eq!(
        document.cmyk_profile().unwrap().source(),
        CmykSource::OutputIntent
    );
    // Cyan ink is neutral in this profile, black ink alone darkens it.
    assert_rgb(cmyk(&document, [1.0, 0.0, 0.0, 0.0]), [1.0; 3], TOLERANCE);
    assert_rgb(cmyk(&document, [0.0, 1.0, 1.0, 0.0]), [1.0; 3], TOLERANCE);
    assert_rgb(cmyk(&document, [0.0, 0.0, 0.0, 1.0]), [0.0; 3], TOLERANCE);
    let half = grey_by_black(0.5);
    assert_rgb(cmyk(&document, [0.0, 0.0, 0.0, 0.5]), [half; 3], TOLERANCE);
    assert_rgb(cmyk(&document, [1.0, 1.0, 1.0, 0.5]), [half; 3], TOLERANCE);
    assert!(of_kind(&document, &DiagnosticKind::IccProfileUnusable).is_empty());
}

/// A DeviceCMYK colour that is current when a stream says `/DeviceCMYK cs`
/// is black, whatever the profile: 0 0 0 1.
#[test]
fn the_initial_colour_is_black_ink() {
    let document = with_intents(&[&grey_by_black_profile()]);
    let space = document.device_cmyk().unwrap();
    assert_eq!(space.initial_colour(), [0.0, 0.0, 0.0, 1.0]);
    assert_rgb(
        space.to_rgb(&space.initial_colour()).unwrap(),
        [0.0; 3],
        TOLERANCE,
    );
}

/// The intents of a PDF/A file are often an sRGB profile. It is not a CMYK
/// intent: nothing is wrong with it, so nothing is reported, and the
/// bundled profile is used.
#[test]
fn an_intent_that_is_not_cmyk_is_passed_over() {
    let document = with_intents(&[&srgb_profile()]);
    assert_eq!(
        document.cmyk_profile().unwrap().source(),
        CmykSource::Bundled
    );
    assert!(of_kind(&document, &DiagnosticKind::IccProfileUnusable).is_empty());
}

/// The first intent that gives a CMYK profile that can be used wins; one
/// that is not CMYK before it is passed over.
#[test]
fn the_first_usable_cmyk_intent_wins() {
    let document = with_intents(&[&srgb_profile(), &grey_by_black_profile(), &srgb_profile()]);
    assert_eq!(
        document.cmyk_profile().unwrap().source(),
        CmykSource::OutputIntent
    );
    assert_rgb(cmyk(&document, [1.0, 0.0, 0.0, 0.0]), [1.0; 3], TOLERANCE);
}

/// A CMYK profile that cannot be used is reported, and the next intent is
/// tried.
#[test]
fn an_unusable_cmyk_intent_is_reported_and_the_next_is_tried() {
    let document = with_intents(&[&broken_cmyk_profile(), &grey_by_black_profile()]);
    assert_eq!(
        document.cmyk_profile().unwrap().source(),
        CmykSource::OutputIntent
    );
    assert_rgb(cmyk(&document, [1.0, 0.0, 0.0, 0.0]), [1.0; 3], TOLERANCE);
    assert_eq!(
        of_kind(&document, &DiagnosticKind::IccProfileUnusable).len(),
        1
    );
}

/// With no other intent, an unusable one is reported and the chain goes
/// on: the bundled profile. Reading it twice reports it once.
#[test]
fn an_unusable_only_intent_falls_through_to_the_bundled_profile() {
    let document = with_intents(&[&broken_cmyk_profile()]);
    assert_eq!(
        document.cmyk_profile().unwrap().source(),
        CmykSource::Bundled
    );
    let _ = document.cmyk_profile().unwrap();
    assert_eq!(
        of_kind(&document, &DiagnosticKind::IccProfileUnusable).len(),
        1
    );
}

/// A document whose catalog carries `entry` (`/OutputIntents ...`) beside
/// `objects`.
fn with_catalog_entry(entry: &str, objects: &[(u32, &[u8])]) -> Document {
    let mut all = pages_document(1);
    all[0] = (
        1,
        format!("<< /Type /Catalog /Pages 2 0 R {entry} >>").into_bytes(),
    );
    for (number, body) in objects {
        all.push((*number, body.to_vec()));
    }
    table_file(&borrowed(&all), "/Root 1 0 R /Info 3 0 R").open()
}

/// An object number and its body.
type Extra = (u32, &'static [u8]);

/// DeviceCMYK is converted all over a document, so a damaged output intent
/// must not stop it: the intent is reported once, however often the
/// profile is asked for, and the bundled profile stands in.
#[test]
fn a_damaged_output_intent_is_reported_and_does_not_stop_the_conversion() {
    let intent_of =
        |dictionary: &'static [u8]| -> Vec<(u32, &'static [u8])> { vec![(61, dictionary)] };
    let cases: [(&str, Vec<Extra>); 5] = [
        // The profile is an object the file does not have.
        (
            "/OutputIntents [61 0 R]",
            intent_of(b"<< /DestOutputProfile 99 0 R >>"),
        ),
        // The intent itself is an object the file does not have.
        ("/OutputIntents [98 0 R]", Vec::new()),
        // The profile is not a stream.
        (
            "/OutputIntents [61 0 R]",
            vec![
                (61, b"<< /DestOutputProfile 62 0 R >>"),
                (62, b"<< /N 4 >>"),
            ],
        ),
        // The profile is a number.
        (
            "/OutputIntents [61 0 R]",
            intent_of(b"<< /DestOutputProfile 4 >>"),
        ),
        // The intent is not a dictionary.
        ("/OutputIntents [7]", Vec::new()),
    ];
    for (entry, objects) in cases {
        let document = with_catalog_entry(entry, &objects);
        for _ in 0..2 {
            assert_eq!(
                document.cmyk_profile().unwrap().source(),
                CmykSource::Bundled,
                "{entry}"
            );
            assert!(document
                .device_cmyk()
                .unwrap()
                .to_rgb(&[0.0, 0.0, 0.0, 1.0])
                .is_ok());
        }
        assert_eq!(
            of_kind(&document, &DiagnosticKind::IccProfileUnusable).len(),
            1,
            "{entry}"
        );
    }
}

/// A damaged intent does not hide a good one after it.
#[test]
fn a_damaged_intent_before_a_good_one_is_passed_over() {
    let good = stream(" /N 4", &grey_by_black_profile());
    let document = with_catalog_entry(
        "/OutputIntents [61 0 R 62 0 R]",
        &[
            (61, b"<< /DestOutputProfile 99 0 R >>"),
            (62, b"<< /DestOutputProfile 63 0 R >>"),
            (63, &good),
        ],
    );
    assert_eq!(
        document.cmyk_profile().unwrap().source(),
        CmykSource::OutputIntent
    );
    assert_eq!(
        of_kind(&document, &DiagnosticKind::IccProfileUnusable).len(),
        1
    );
}

/// An intent with no `DestOutputProfile` names a registered production
/// condition (14.11.5): it is not damaged, so nothing is reported.
#[test]
fn an_intent_without_a_profile_is_not_reported() {
    let document = with_catalog_entry(
        "/OutputIntents [61 0 R]",
        &[(
            61,
            b"<< /Type /OutputIntent /S /GTS_PDFX /OutputConditionIdentifier (FOGRA39) >>",
        )],
    );
    assert_eq!(
        document.cmyk_profile().unwrap().source(),
        CmykSource::Bundled
    );
    assert!(of_kind(&document, &DiagnosticKind::IccProfileUnusable).is_empty());
}

/// A snapshot made by an edit reads an intent's profile once for the file:
/// what was reported is not reported again, and the transform is the
/// same. An edit of the profile stream itself is seen.
#[test]
fn an_edit_does_not_read_the_intent_profile_again_unless_it_touched_it() {
    let document = with_intents(&[&broken_cmyk_profile()]);
    assert_eq!(
        document.cmyk_profile().unwrap().source(),
        CmykSource::Bundled
    );
    let mut builder = ObjectBuilder::new(&document);
    builder.add(Object::new(Value::Integer(1))).unwrap();
    let elsewhere = document.apply(builder.finish()).unwrap();
    assert_eq!(
        elsewhere.cmyk_profile().unwrap().source(),
        CmykSource::Bundled
    );
    assert_eq!(
        of_kind(&elsewhere, &DiagnosticKind::IccProfileUnusable).len(),
        1
    );
    // The stream of the intent, object 70, is replaced by a good profile.
    let mut builder = ObjectBuilder::new(&document);
    builder
        .set_stream(
            id(70),
            kernkraft_pdf::syntax::Dictionary::new(),
            grey_by_black_profile(),
            kernkraft_pdf::doc::StreamEncoding::AsGiven,
        )
        .unwrap();
    let touched = document.apply(builder.finish()).unwrap();
    assert_eq!(
        touched.cmyk_profile().unwrap().source(),
        CmykSource::OutputIntent
    );
    assert_rgb(cmyk(&touched, [1.0, 0.0, 0.0, 0.0]), [1.0; 3], TOLERANCE);
    // The first snapshot is as it was.
    assert_eq!(
        document.cmyk_profile().unwrap().source(),
        CmykSource::Bundled
    );
}

// The profile of the application.

#[test]
fn the_application_profile_is_used_when_the_file_has_no_intent() {
    let document = document(&[]);
    document.set_cmyk_profile(&grey_by_black_profile()).unwrap();
    assert_eq!(
        document.cmyk_profile().unwrap().source(),
        CmykSource::Application
    );
    assert_rgb(cmyk(&document, [1.0, 0.0, 0.0, 0.0]), [1.0; 3], TOLERANCE);
}

#[test]
fn an_output_intent_comes_before_the_application_profile() {
    let document = with_intents(&[&grey_by_black_profile()]);
    // The application's profile is the bundled one, which makes cyan
    // coloured: the neutral cyan below is the intent's.
    document.set_cmyk_profile(BUNDLED_PROFILE).unwrap();
    assert_eq!(
        document.cmyk_profile().unwrap().source(),
        CmykSource::OutputIntent
    );
    assert_rgb(cmyk(&document, [1.0, 0.0, 0.0, 0.0]), [1.0; 3], TOLERANCE);
}

#[test]
fn an_unusable_intent_leaves_the_application_profile_in_force() {
    let document = with_intents(&[&broken_cmyk_profile()]);
    document.set_cmyk_profile(&grey_by_black_profile()).unwrap();
    assert_eq!(
        document.cmyk_profile().unwrap().source(),
        CmykSource::Application
    );
}

/// A profile the application gives that cannot be used is an error, and
/// the profile in force stays as it was.
#[test]
fn an_application_profile_that_cannot_be_used_is_refused() {
    let document = document(&[]);
    for profile in [
        srgb_profile(),
        broken_cmyk_profile(),
        b"not a profile".to_vec(),
        Vec::new(),
    ] {
        assert!(
            matches!(
                document.set_cmyk_profile(&profile),
                Err(PdfError::Invalid(_))
            ),
            "{} bytes",
            profile.len()
        );
    }
    assert_eq!(
        document.cmyk_profile().unwrap().source(),
        CmykSource::Bundled
    );
}

/// The profile belongs to the opened file: a snapshot made by an edit, and
/// a clone, convert through it.
#[test]
fn the_application_profile_follows_the_document_through_edits() {
    let document = document(&[]);
    document.set_cmyk_profile(&grey_by_black_profile()).unwrap();
    let mut builder = ObjectBuilder::new(&document);
    builder.add(Object::new(Value::Integer(1))).unwrap();
    let edited = document.apply(builder.finish()).unwrap();
    assert_eq!(
        edited.cmyk_profile().unwrap().source(),
        CmykSource::Application
    );
    assert_rgb(cmyk(&edited, [1.0, 0.0, 0.0, 0.0]), [1.0; 3], TOLERANCE);
    assert_eq!(
        document.clone().cmyk_profile().unwrap().source(),
        CmykSource::Application
    );
}

// Every DeviceCMYK conversion of a document follows its profile.

/// 8.6.6.4: a Separation's tint goes through its transform into the
/// alternate space, and so through the document's CMYK profile.
#[test]
fn a_cmyk_alternate_follows_the_intent() {
    let transform = b"<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [0 1 1 0] /N 1 >>";
    let space_of_document = |profile: Option<&[u8]>| {
        let mut objects = pages_document(1);
        objects.push((60, b"[/Separation /Spot /DeviceCMYK 61 0 R]".to_vec()));
        objects.push((61, transform.to_vec()));
        if let Some(profile) = profile {
            objects.push((62, b"<< /DestOutputProfile 63 0 R >>".to_vec()));
            objects.push((63, stream(" /N 4", profile)));
            objects[0] = (
                1,
                b"<< /Type /Catalog /Pages 2 0 R /OutputIntents [62 0 R] >>".to_vec(),
            );
        }
        let document = table_file(&borrowed(&objects), "/Root 1 0 R /Info 3 0 R").open();
        let space = document.colour_space(&reference(60)).unwrap();
        (document, space)
    };
    // With the grey profile, magenta and yellow are neutral: full tint is
    // white, as the paper is.
    let (_, space) = space_of_document(Some(&grey_by_black_profile()));
    assert_rgb(rgb(&space, &[1.0]), [1.0; 3], TOLERANCE);
    // Without an intent, the tint (0 1 1 0) is a red by the bundled
    // profile: more red than green or blue.
    let (_, space) = space_of_document(None);
    let red = rgb(&space, &[1.0]);
    assert!(red.r > red.g && red.r > red.b, "{red:?}");
    assert_rgb(red, through(BUNDLED_PROFILE, [0.0, 1.0, 1.0, 0.0]), 1e-6);
    assert_rgb(rgb(&space, &[0.0]), [1.0; 3], 0.02);
}

fn rgb(space: &ColourSpace, colour: &[f64]) -> Rgb {
    space.to_rgb(colour).unwrap()
}

/// 8.6.5.6: a page's `DefaultCMYK` replaces DeviceCMYK before any intent
/// is looked at.
#[test]
fn a_default_cmyk_space_comes_before_the_intent() {
    let mut objects = pages_document(1);
    objects.push((
        60,
        b"<< /ColorSpace << /DefaultCMYK [/ICCBased 61 0 R] >> >>".to_vec(),
    ));
    objects.push((61, stream(" /N 4", &grey_by_black_profile())));
    objects.push((62, b"<< /DestOutputProfile 63 0 R >>".to_vec()));
    objects.push((63, stream(" /N 4", BUNDLED_PROFILE)));
    objects[0] = (
        1,
        b"<< /Type /Catalog /Pages 2 0 R /OutputIntents [62 0 R] >>".to_vec(),
    );
    let document = table_file(&borrowed(&objects), "/Root 1 0 R /Info 3 0 R").open();
    let resources = document
        .resolve(&reference(60))
        .unwrap()
        .dictionary()
        .cloned()
        .unwrap();
    let defaults = document.default_colour_spaces(&resources).unwrap();
    let device = document.device_cmyk().unwrap();
    let replaced = defaults.apply(&device);
    assert!(matches!(replaced, ColourSpace::IccBased(_)));
    // The page's default is the hand-written profile, the intent is the
    // bundled one: cyan is neutral only in the first.
    assert_rgb(rgb(replaced, &[1.0, 0.0, 0.0, 0.0]), [1.0; 3], TOLERANCE);
    assert_eq!(
        document.cmyk_profile().unwrap().source(),
        CmykSource::OutputIntent
    );
}

/// An image's colours are converted in one pass: the same as one by one,
/// out of the range as well as in it, and NaN is no ink.
#[test]
fn many_colours_convert_as_one_by_one() {
    let document = with_intents(&[&grey_by_black_profile()]);
    let space = document.device_cmyk().unwrap();
    let colours = [
        0.0,
        0.0,
        0.0,
        0.0, // no ink
        1.0,
        0.5,
        0.25,
        0.5, // inks and black
        0.0,
        0.0,
        0.0,
        1.0, // black
        2.0,
        -1.0,
        0.0,
        0.25, // outside 0 to 1
        f64::NAN,
        0.0,
        0.0,
        f64::NAN, // not a number
    ];
    let mut together = Vec::new();
    space.to_rgb_many(&colours, &mut together).unwrap();
    assert_eq!(together.len(), colours.len() / 4);
    for (colour, got) in colours.as_chunks::<4>().0.iter().zip(&together) {
        let one = space.to_rgb(colour).unwrap();
        assert_rgb(*got, [one.r, one.g, one.b], 1e-12);
    }
    // NaN is no ink; 2 and -1 are clipped to 1 and 0.
    assert_rgb(together[4], [1.0; 3], TOLERANCE);
    assert_rgb(together[3], [grey_by_black(0.25); 3], TOLERANCE);
    // A count that is not a whole number of colours is refused.
    assert!(space.to_rgb_many(&colours[..5], &mut together).is_err());
}

/// The profile file of `testdata/icc`, which the adapters' tests use, is
/// what the builder writes.
#[test]
fn the_profile_file_of_the_test_data_is_the_one_the_builder_writes() {
    let file = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/icc/cmyk-grey-by-black.icc"
    ))
    .unwrap();
    assert_eq!(file, grey_by_black_profile());
}

/// The profile of the application from a file, and from bytes it was made
/// from: the file's, whatever was passed, is the application's.
#[test]
fn a_profile_is_read_from_a_file_and_given_to_a_document() {
    let path = std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/icc/cmyk-grey-by-black.icc"
    ));
    let profile = CmykProfile::from_file(path).unwrap();
    assert_eq!(profile.source(), CmykSource::Application);
    let document = document(&[]);
    document.use_cmyk_profile(profile);
    assert_eq!(
        document.cmyk_profile().unwrap().source(),
        CmykSource::Application
    );
    assert_rgb(cmyk(&document, [1.0, 0.0, 0.0, 0.0]), [1.0; 3], TOLERANCE);
    // The bundled profile given as the application's is the application's.
    document.use_cmyk_profile(CmykProfile::bundled().unwrap());
    assert_eq!(
        document.cmyk_profile().unwrap().source(),
        CmykSource::Application
    );
}

#[test]
fn a_profile_file_that_cannot_be_used_is_an_error() {
    let directory = tempfile::tempdir().unwrap();
    // A missing file.
    assert!(matches!(
        CmykProfile::from_file(&directory.path().join("missing.icc")),
        Err(PdfError::Fs(_))
    ));
    // An sRGB profile, and a file that says CMYK and is not a profile.
    for (name, bytes) in [
        ("rgb.icc", srgb_profile()),
        ("broken.icc", broken_cmyk_profile()),
    ] {
        let path = directory.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        assert!(matches!(
            CmykProfile::from_file(&path),
            Err(PdfError::Invalid(_))
        ));
    }
    // A file past the largest profile is refused, having been read only as
    // far as that.
    let path = directory.path().join("large.icc");
    let mut large = broken_cmyk_profile();
    large.resize((10 << 20) + 1, 0);
    std::fs::write(&path, large).unwrap();
    assert!(matches!(
        CmykProfile::from_file(&path),
        Err(PdfError::Invalid(_))
    ));
}
