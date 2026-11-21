// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The image API of a document: `image_xobject` and `inline_image`, which
//! give the pixels of an image and its masks without drawing a page.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

mod pdf_fixture;

use kernkraft_pdf::image::{DecodedImage, ImageOptions};
use kernkraft_pdf::syntax::{Dictionary, Name, Object, ObjectId, Parser, Value};
use kernkraft_pdf::{DiagnosticKind, Diagnostics, ImageFault, MemorySource};
use pdf_fixture::*;

const IMAGE: ObjectId = ObjectId {
    number: 20,
    generation: 0,
};

fn image_object(entries: &str, data: &[u8]) -> Vec<u8> {
    stream(&format!(" /Type /XObject /Subtype /Image {entries}"), data)
}

fn open(extra: &[(u32, Vec<u8>)]) -> kernkraft_pdf::doc::Document {
    let mut objects: Vec<(u32, Vec<u8>)> = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()),
        (
            3,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 20 20] >>".to_vec(),
        ),
    ];
    objects.extend(extra.iter().cloned());
    table_file(&borrowed(&objects), "/Root 1 0 R").open()
}

fn decode(doc: &kernkraft_pdf::doc::Document) -> Option<DecodedImage> {
    doc.image_xobject(IMAGE, &ImageOptions::default()).unwrap()
}

fn colour(image: DecodedImage) -> kernkraft_pdf::image::Bitmap {
    match image {
        DecodedImage::Colour(bitmap) => bitmap,
        other => panic!("not a picture: {other:?}"),
    }
}

fn faults(doc: &kernkraft_pdf::doc::Document) -> Vec<DiagnosticKind> {
    doc.diagnostics()
        .into_iter()
        .map(|d| d.kind)
        .filter(|k| {
            matches!(
                k,
                DiagnosticKind::ImageUnusable { .. }
                    | DiagnosticKind::ImageMaskUnusable { .. }
                    | DiagnosticKind::ImageDataShort
                    | DiagnosticKind::ImageSizeDiffers
            )
        })
        .collect()
}

#[test]
fn a_picture_is_straight_rgba_row_by_row_from_the_top() {
    let doc = open(&[(
        20,
        image_object(
            "/Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8",
            &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12],
        ),
    )]);
    let bitmap = colour(decode(&doc).unwrap());
    assert_eq!((bitmap.width, bitmap.height), (2, 2));
    assert_eq!(
        bitmap.rgba,
        [1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255]
    );
    assert!(!bitmap.interpolate);
    assert!(faults(&doc).is_empty());
}

#[test]
fn interpolate_is_carried() {
    let doc = open(&[(
        20,
        image_object(
            "/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Interpolate true",
            &[7],
        ),
    )]);
    assert!(colour(decode(&doc).unwrap()).interpolate);
}

#[test]
fn a_stencil_is_coverage_with_the_sample_that_paints_at_255() {
    let doc = open(&[(
        20,
        image_object(
            "/Width 3 /Height 1 /ImageMask true /BitsPerComponent 1",
            &[0b0100_0000],
        ),
    )]);
    match decode(&doc).unwrap() {
        DecodedImage::Stencil(stencil) => {
            assert_eq!((stencil.width, stencil.height), (3, 1));
            assert_eq!(stencil.coverage, [255, 0, 255]);
        }
        other => panic!("not a stencil: {other:?}"),
    }
}

#[test]
fn a_soft_mask_becomes_the_alpha_at_the_larger_size() {
    let doc = open(&[
        (
            20,
            image_object(
                "/Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /SMask 21 0 R",
                &[10, 20, 30],
            ),
        ),
        (
            21,
            image_object(
                "/Width 2 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8",
                &[255, 64],
            ),
        ),
    ]);
    let bitmap = colour(decode(&doc).unwrap());
    assert_eq!((bitmap.width, bitmap.height), (2, 1));
    assert_eq!(bitmap.rgba, [10, 20, 30, 255, 10, 20, 30, 64]);
}

#[test]
fn a_soft_mask_decode_array_inverts_it() {
    let doc = open(&[
        (
            20,
            image_object(
                "/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /SMask 21 0 R",
                &[200],
            ),
        ),
        (
            21,
            image_object(
                "/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Decode [1 0]",
                &[255],
            ),
        ),
    ]);
    assert_eq!(colour(decode(&doc).unwrap()).rgba, [200, 200, 200, 0]);
}

#[test]
fn a_soft_mask_with_no_dimensions_leaves_the_picture_unmasked_and_says_so() {
    let doc = open(&[
        (
            20,
            image_object(
                "/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /SMask 21 0 R",
                &[200],
            ),
        ),
        (
            21,
            stream("/Type /XObject /Subtype /Form /BBox [0 0 1 1]", b""),
        ),
    ]);
    assert_eq!(colour(decode(&doc).unwrap()).rgba, [200, 200, 200, 255]);
    assert!(faults(&doc).iter().any(|k| matches!(
        k,
        DiagnosticKind::ImageMaskUnusable {
            fault: ImageFault::Dimensions
        }
    )));
}

#[test]
fn an_explicit_mask_that_is_not_an_image_mask_is_ignored_and_said() {
    let doc = open(&[
        (
            20,
            image_object(
                "/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Mask 21 0 R",
                &[200],
            ),
        ),
        (
            21,
            image_object(
                "/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8",
                &[0],
            ),
        ),
    ]);
    assert_eq!(colour(decode(&doc).unwrap()).rgba, [200, 200, 200, 255]);
    assert!(faults(&doc).iter().any(|k| matches!(
        k,
        DiagnosticKind::ImageMaskUnusable {
            fault: ImageFault::NotAnImage
        }
    )));
}

#[test]
fn a_decode_array_of_the_wrong_length_is_ignored_and_said() {
    let doc = open(&[(
        20,
        image_object(
            "/Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Decode [1 0]",
            &[1, 2, 3],
        ),
    )]);
    assert_eq!(colour(decode(&doc).unwrap()).rgba, [1, 2, 3, 255]);
    assert!(doc.diagnostics().iter().any(
        |d| matches!(&d.kind, DiagnosticKind::IgnoredEntry { key } if key.as_bytes() == b"Decode")
    ));
}

#[test]
fn bits_per_component_that_is_not_allowed_leaves_the_image_out() {
    let doc = open(&[(
        20,
        image_object(
            "/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 3",
            &[0],
        ),
    )]);
    assert!(decode(&doc).is_none());
    assert_eq!(
        faults(&doc),
        [DiagnosticKind::ImageUnusable {
            fault: ImageFault::Dimensions
        }]
    );
}

#[test]
fn an_object_that_is_not_an_image_stream_is_none() {
    let doc = open(&[(20, b"<< /Width 1 >>".to_vec())]);
    assert!(decode(&doc).is_none());
    assert!(doc
        .image_xobject(
            ObjectId {
                number: 99,
                generation: 0
            },
            &ImageOptions::default()
        )
        .unwrap()
        .is_none());
}

#[test]
fn sixteen_bit_samples_use_the_high_byte() {
    let doc = open(&[(
        20,
        image_object(
            "/Width 2 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 16",
            &[0x80, 0x00, 0xFF, 0xFF],
        ),
    )]);
    assert_eq!(
        colour(decode(&doc).unwrap()).rgba,
        [128, 128, 128, 255, 255, 255, 255, 255]
    );
}

#[test]
fn an_inline_image_takes_its_dictionary_and_data() {
    let source = MemorySource::new(
        b"<< /Width 2 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 >>".to_vec(),
    );
    let object = Parser::new(&source)
        .parse_object(&mut Diagnostics::new())
        .unwrap();
    let Value::Dictionary(dictionary) = object.value() else {
        panic!("not a dictionary");
    };
    let doc = open(&[]);
    let image = doc
        .inline_image(dictionary, &[10, 250], &ImageOptions::default())
        .unwrap()
        .unwrap();
    assert_eq!(colour(image).rgba, [10, 10, 10, 255, 250, 250, 250, 255]);
}

#[test]
fn an_inline_image_may_not_name_a_resource_colour_space_here() {
    let mut dictionary = Dictionary::new();
    for (key, value) in [
        ("Width", Value::Integer(1)),
        ("Height", Value::Integer(1)),
        ("BitsPerComponent", Value::Integer(8)),
        ("ColorSpace", Value::Name(Name::new(b"Cs1".to_vec()))),
    ] {
        dictionary.insert(Name::new(key.as_bytes().to_vec()), Object::new(value));
    }
    let doc = open(&[]);
    assert!(doc
        .inline_image(&dictionary, &[0], &ImageOptions::default())
        .unwrap()
        .is_none());
}
