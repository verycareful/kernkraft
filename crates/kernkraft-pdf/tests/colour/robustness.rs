// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Deterministic mutation of a file of functions and colour spaces: every
//! truncation and every substitution of one byte by a byte that means
//! something to the reader. Each mutated file must open or be refused, and
//! every function and colour space in it must read or be refused, and
//! evaluate or fail with an error: a panic or a hang fails the test.

use super::*;

/// Bytes that change how the reader takes what follows.
const REPLACEMENTS: &[u8] = b"()<>[]{}/% \n\x0009Rnf-.\xFF";

fn file() -> Vec<u8> {
    let objects: Vec<(u32, Vec<u8>)> = vec![
        (60, b"<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [0 1 1 0] /N 1 >>".to_vec()),
        (
            61,
            stream(
                " /FunctionType 0 /Domain [0 1 0 1] /Range [0 1 0 1 0 1] /Size [2 2] \
                 /BitsPerSample 8 /Order 3",
                &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
            ),
        ),
        (
            62,
            stream(
                " /FunctionType 4 /Domain [0 1 0 1] /Range [0 1 0 1 0 1]",
                b"{ 2 copy lt { exch } if 360 mul sin 2 div 1 index 3 1 roll pop 0.5 }",
            ),
        ),
        (
            63,
            b"<< /FunctionType 3 /Domain [0 1] /Functions [60 0 R 60 0 R] /Bounds [0.5] \
              /Encode [0 1 1 0] >>"
                .to_vec(),
        ),
        (64, b"[/Separation /S /DeviceCMYK 60 0 R]".to_vec()),
        (65, b"[/DeviceN [/A /B] /DeviceRGB 62 0 R << /Subtype /NChannel >>]".to_vec()),
        (66, b"[/Indexed [/CalRGB << /WhitePoint [0.95 1 1.09] /Gamma [2 2 2] >>] 2 <000000 FF8000 00FF80>]".to_vec()),
        (67, b"[/Lab << /WhitePoint [0.96 1 0.82] /Range [-100 100 -100 100] >>]".to_vec()),
        (68, b"[/ICCBased 69 0 R]".to_vec()),
        (69, stream(" /N 3 /Alternate /DeviceRGB", b"not a profile")),
        (70, b"[/Pattern [/CalGray << /WhitePoint [0.95 1 1.09] /Gamma 2 >>]]".to_vec()),
    ];
    let mut all = pages_document(1);
    all.extend(objects);
    table_file(&borrowed(&all), "/Root 1 0 R /Info 3 0 R").bytes
}

fn exercise(bytes: &[u8]) {
    let Ok(document) = Document::from_bytes(bytes.to_vec(), &OPTIONS, "") else {
        return;
    };
    for number in [60, 61, 62, 63] {
        if let Ok(function) = document.function(&reference(number)) {
            let input = vec![0.25; function.inputs()];
            let _ = function.call(&input);
            let _ = function.call(&vec![-5.0; function.inputs()]);
            let _ = function.call(&vec![5.0; function.inputs()]);
        }
    }
    for number in [64, 65, 66, 67, 68, 70] {
        if let Ok(space) = document.colour_space(&reference(number)) {
            let colour = space.initial_colour();
            let _ = space.to_rgb(&colour);
            let _ = space.to_rgb(&vec![0.5; space.components()]);
            let _ = space.to_rgb(&vec![f64::INFINITY; space.components()]);
            let _ = space.default_decode(8);
        }
    }
}

#[test]
fn the_unmutated_file_reads() {
    let document = Document::from_bytes(file(), &OPTIONS, "").unwrap();
    for number in [60, 61, 62, 63] {
        document.function(&reference(number)).unwrap();
    }
    for number in [64, 65, 66, 67, 68, 70] {
        let space = document.colour_space(&reference(number)).unwrap();
        space.to_rgb(&space.initial_colour()).unwrap();
    }
}

#[test]
fn every_truncation_is_survived() {
    let bytes = file();
    let step = 7;
    for end in (0..bytes.len()).step_by(step) {
        exercise(&bytes[..end]);
    }
}

#[test]
fn every_substitution_is_survived() {
    let bytes = file();
    // The objects are in the file's first part: mutate the bytes from the
    // first of ours on.
    let start = bytes
        .windows(8)
        .position(|window| window == b"60 0 obj")
        .unwrap();
    let end = bytes.len().min(start + 1400);
    for position in (start..end).step_by(3) {
        for replacement in REPLACEMENTS {
            let mut mutated = bytes.clone();
            mutated[position] = *replacement;
            exercise(&mutated);
        }
    }
}
