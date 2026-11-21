// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Functions (7.10): types 0, 2, 3 and 4.

use super::*;
use kernkraft_pdf::colour::FunctionFault;
use kernkraft_pdf::Limit;

fn fault(error: PdfError) -> FunctionFault {
    match error {
        PdfError::Function(fault) => fault,
        other => panic!("not a function fault: {other:?}"),
    }
}

// --- type 2 (7.10.3) ---

fn exponential(entries: &str) -> Vec<u8> {
    format!("<< /FunctionType 2 /Domain [0 1] {entries} >>").into_bytes()
}

/// Table 40: `y_j = C0_j + x^N * (C1_j - C0_j)`.
#[test]
fn an_exponential_function_follows_its_formula() {
    let f = function_of(&[(60, &exponential("/C0 [0 0] /C1 [1 0.5] /N 2"))]).unwrap();
    assert_eq!((f.inputs(), f.outputs()), (1, 2));
    // 0.5^2 = 0.25: 0 + 0.25 * 1 and 0 + 0.25 * 0.5.
    assert_eq!(f.call(&[0.5]).unwrap(), [0.25, 0.125]);
    assert_eq!(f.call(&[0.0]).unwrap(), [0.0, 0.0]);
    assert_eq!(f.call(&[1.0]).unwrap(), [1.0, 0.5]);
}

/// Table 40: `C0` defaults to `[0.0]`, `C1` to `[1.0]`.
#[test]
fn exponential_defaults_give_one_output_from_zero_to_one() {
    let f = function_of(&[(60, &exponential("/N 1"))]).unwrap();
    assert_eq!(f.outputs(), 1);
    assert_eq!(f.call(&[0.3]).unwrap(), [0.3]);
}

/// Table 38: an input is clipped to the domain and an output to the range.
#[test]
fn input_and_output_are_clipped() {
    let f = function_of(&[(60, &exponential("/C0 [0] /C1 [4] /N 1 /Range [0 2]"))]).unwrap();
    assert_eq!(f.call(&[7.0]).unwrap(), [2.0]);
    assert_eq!(f.call(&[-3.0]).unwrap(), [0.0]);
    assert_eq!(f.call(&[0.25]).unwrap(), [1.0]);
}

/// 7.10.3: a fractional exponent of a negative number, or a negative
/// exponent of zero, has no value.
#[test]
fn an_exponential_power_with_no_value_is_undefined() {
    let body = b"<< /FunctionType 2 /Domain [-1 1] /N 0.5 >>".to_vec();
    let f = function_of(&[(60, &body)]).unwrap();
    assert_eq!(
        fault(f.call(&[-0.5]).unwrap_err()),
        FunctionFault::Undefined
    );
    let body = b"<< /FunctionType 2 /Domain [0 1] /N -1 >>".to_vec();
    let f = function_of(&[(60, &body)]).unwrap();
    assert_eq!(fault(f.call(&[0.0]).unwrap_err()), FunctionFault::Undefined);
}

#[test]
fn the_wrong_number_of_inputs_is_refused() {
    let f = function_of(&[(60, &exponential("/N 1"))]).unwrap();
    assert_eq!(
        fault(f.call(&[0.1, 0.2]).unwrap_err()),
        FunctionFault::InputCount {
            expected: 1,
            found: 2
        }
    );
}

#[test]
fn a_function_with_a_missing_or_wrong_entry_is_refused() {
    for (body, key) in [
        ("<< /FunctionType 2 /Domain [0 1] >>", "N"),
        ("<< /FunctionType 2 /N 1 >>", "Domain"),
        ("<< /FunctionType 2 /Domain [0 1 0] /N 1 >>", "Domain"),
        (
            "<< /FunctionType 2 /Domain [0 1] /N 1 /C0 [0] /C1 [0 1] >>",
            "C1",
        ),
        ("<< /FunctionType 9 /Domain [0 1] >>", "FunctionType"),
        ("<< /Domain [0 1] >>", "FunctionType"),
    ] {
        match function_of(&[(60, body.as_bytes())]) {
            Err(PdfError::InvalidEntry { key: found, .. }) => assert_eq!(found, key, "{body}"),
            other => panic!("{body}: {other:?}"),
        }
    }
}

// --- type 0 (7.10.2) ---

fn sampled(entries: &str, data: &[u8]) -> Vec<u8> {
    stream(&format!(" /FunctionType 0 {entries}"), data)
}

/// Three samples 0, 255, 0 of 8 bits over a domain of 0 to 1: the encoded
/// input 0 to 2 is blended between neighbours, and the decode is 0 to 1.
#[test]
fn a_sampled_function_interpolates_between_samples() {
    let body = sampled(
        "/Domain [0 1] /Range [0 1] /Size [3] /BitsPerSample 8",
        &[0, 255, 0],
    );
    let f = function_of(&[(60, &body)]).unwrap();
    near(f.call(&[0.25]).unwrap()[0], 0.5, 1e-12); // e = 0.5
    near(f.call(&[0.5]).unwrap()[0], 1.0, 1e-12); // e = 1
    near(f.call(&[0.75]).unwrap()[0], 0.5, 1e-12); // e = 1.5
    near(f.call(&[1.0]).unwrap()[0], 0.0, 1e-12);
}

/// 7.10.2: the first dimension varies fastest. f(x, y) = x has samples
/// f(0,0) f(1,0) f(0,1) f(1,1) = 0 1 0 1; f(x, y) = y has 0 0 1 1.
#[test]
fn the_first_input_varies_fastest_in_the_table() {
    let entries = "/Domain [0 1 0 1] /Range [0 1] /Size [2 2] /BitsPerSample 8";
    let x = function_of(&[(60, &sampled(entries, &[0, 255, 0, 255]))]).unwrap();
    let y = function_of(&[(60, &sampled(entries, &[0, 0, 255, 255]))]).unwrap();
    near(x.call(&[0.25, 0.7]).unwrap()[0], 0.25, 1e-12);
    near(y.call(&[0.25, 0.7]).unwrap()[0], 0.7, 1e-12);
    // Bilinear: the centre of a cell is the mean of its corners.
    let diagonal = function_of(&[(60, &sampled(entries, &[0, 85, 85, 255]))]).unwrap();
    near(
        diagonal.call(&[0.5, 0.5]).unwrap()[0],
        (0.0 + 85.0 + 85.0 + 255.0) / 4.0 / 255.0,
        1e-12,
    );
}

/// 7.10.2: samples of 4 and of 12 bits are adjacent in the bit stream,
/// high-order bit first, with no padding at byte boundaries.
#[test]
fn samples_of_any_valid_width_are_unpacked() {
    // 4 bits: 0, 5, 10, 15 are the bytes 0x05 0xAF.
    let f = function_of(&[(
        60,
        &sampled(
            "/Domain [0 1] /Range [0 15] /Size [4] /BitsPerSample 4 /Decode [0 15]",
            &[0x05, 0xAF],
        ),
    )])
    .unwrap();
    for (x, expected) in [(0.0, 0.0), (1.0 / 3.0, 5.0), (2.0 / 3.0, 10.0), (1.0, 15.0)] {
        near(f.call(&[x]).unwrap()[0], expected, 1e-9);
    }
    // 12 bits: 0x000 and 0xFFF are the three bytes 00 0F FF.
    let f = function_of(&[(
        60,
        &sampled(
            "/Domain [0 1] /Range [0 1] /Size [2] /BitsPerSample 12",
            &[0x00, 0x0F, 0xFF],
        ),
    )])
    .unwrap();
    near(f.call(&[0.5]).unwrap()[0], 0.5, 1e-12);
    // 16 and 32 bits.
    let f = function_of(&[(
        60,
        &sampled(
            "/Domain [0 1] /Range [0 1] /Size [2] /BitsPerSample 32",
            &[0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF],
        ),
    )])
    .unwrap();
    near(f.call(&[0.25]).unwrap()[0], 0.25, 1e-12);
}

/// Several outputs are stored together, in the order of `Range`; `Decode`
/// maps the samples to the outputs and `Encode` the input to the table.
#[test]
fn outputs_decode_and_encode_follow_their_arrays() {
    // Samples (0, 255) and (255, 0).
    let data = [0, 255, 255, 0];
    let f = function_of(&[(
        60,
        &sampled(
            "/Domain [0 1] /Range [0 1 0 1] /Size [2] /BitsPerSample 8",
            &data,
        ),
    )])
    .unwrap();
    let y = f.call(&[0.25]).unwrap();
    near(y[0], 0.25, 1e-12);
    near(y[1], 0.75, 1e-12);
    // Decode [1 0] turns the first output over.
    let f = function_of(&[(
        60,
        &sampled(
            "/Domain [0 1] /Range [0 1 0 1] /Size [2] /BitsPerSample 8 /Decode [1 0 0 1]",
            &data,
        ),
    )])
    .unwrap();
    near(f.call(&[0.25]).unwrap()[0], 0.75, 1e-12);
    // Encode [1 0] reads the table backwards.
    let f = function_of(&[(
        60,
        &sampled(
            "/Domain [0 1] /Range [0 1 0 1] /Size [2] /BitsPerSample 8 /Encode [1 0]",
            &data,
        ),
    )])
    .unwrap();
    near(f.call(&[0.25]).unwrap()[0], 0.75, 1e-12);
}

/// The sample data is shorter than `Size`, `Range` and `BitsPerSample`
/// need: the missing samples are zero and the file is reported.
#[test]
fn a_short_sample_table_is_zero_filled_and_reported() {
    let document = document(&[(
        60,
        &sampled(
            "/Domain [0 1] /Range [0 1] /Size [3] /BitsPerSample 8",
            &[255],
        ),
    )]);
    let f = document.function(&reference(60)).unwrap();
    near(f.call(&[0.0]).unwrap()[0], 1.0, 1e-12);
    near(f.call(&[1.0]).unwrap()[0], 0.0, 1e-12);
    assert_eq!(
        of_kind(&document, &DiagnosticKind::FunctionSamplesShort).len(),
        1
    );
}

/// Table 39: `Order` 3 is cubic interpolation, which is not done; with
/// fewer than four samples in a dimension it is ignored by the format.
#[test]
fn cubic_order_is_reported_only_where_it_would_apply() {
    let wide = document(&[(
        60,
        &sampled(
            "/Domain [0 1] /Range [0 1] /Size [4] /BitsPerSample 8 /Order 3",
            &[0, 1, 2, 3],
        ),
    )]);
    wide.function(&reference(60)).unwrap();
    assert_eq!(
        of_kind(&wide, &DiagnosticKind::FunctionOrderIgnored).len(),
        1
    );
    let narrow = document(&[(
        60,
        &sampled(
            "/Domain [0 1] /Range [0 1] /Size [3] /BitsPerSample 8 /Order 3",
            &[0, 1, 2],
        ),
    )]);
    narrow.function(&reference(60)).unwrap();
    assert!(of_kind(&narrow, &DiagnosticKind::FunctionOrderIgnored).is_empty());
}

#[test]
fn a_sampled_function_with_a_bad_entry_is_refused() {
    for (entries, key) in [
        ("/Domain [0 1] /Size [3] /BitsPerSample 8", "Range"),
        ("/Domain [0 1] /Range [0 1] /BitsPerSample 8", "Size"),
        (
            "/Domain [0 1] /Range [0 1] /Size [3] /BitsPerSample 3",
            "BitsPerSample",
        ),
        (
            "/Domain [0 1] /Range [0 1] /Size [0] /BitsPerSample 8",
            "Size",
        ),
        (
            "/Domain [0 1] /Range [0 1] /Size [2 2] /BitsPerSample 8",
            "Size",
        ),
        (
            "/Domain [0 1] /Range [0 1] /Size [2] /BitsPerSample 8 /Order 2",
            "Order",
        ),
        (
            "/Domain [0 1] /Range [0 1] /Size [2] /BitsPerSample 8 /Decode [0 1 0 1]",
            "Decode",
        ),
        (
            "/Domain [0 1] /Range [0 1] /Size [2] /BitsPerSample 8 /Encode [0]",
            "Encode",
        ),
    ] {
        match function_of(&[(60, &sampled(entries, &[0, 0, 0, 0]))]) {
            Err(PdfError::InvalidEntry { key: found, .. }) => assert_eq!(found, key, "{entries}"),
            other => panic!("{entries}: {other:?}"),
        }
    }
}

/// A table too large to hold is refused with its limit, and so is a
/// function with more inputs than one evaluation can afford.
#[test]
fn the_limits_of_a_sampled_function_are_enforced() {
    let inputs = 17;
    let domain = "0 1 ".repeat(inputs);
    let size = "2 ".repeat(inputs);
    let body = sampled(
        &format!("/Domain [{domain}] /Range [0 1] /Size [{size}] /BitsPerSample 8"),
        &[0; 8],
    );
    match function_of(&[(60, &body)]) {
        Err(PdfError::LimitExceeded {
            limit: Limit::SampledInputs,
            ..
        }) => {}
        other => panic!("{other:?}"),
    }
    // 16 inputs of 2 samples is 65536 bytes: within the limit.
    let inputs = 16;
    let body = sampled(
        &format!(
            "/Domain [{}] /Range [0 1] /Size [{}] /BitsPerSample 8",
            "0 1 ".repeat(inputs),
            "2 ".repeat(inputs)
        ),
        &vec![255; 1 << inputs],
    );
    let f = function_of(&[(60, &body)]).unwrap();
    near(f.call(&[0.5; 16]).unwrap()[0], 1.0, 1e-12);
    // 100000 samples in each of 2 dimensions is 10^10 samples.
    let body = sampled(
        "/Domain [0 1 0 1] /Range [0 1] /Size [100000 100000] /BitsPerSample 8",
        &[0],
    );
    match function_of(&[(60, &body)]) {
        Err(PdfError::LimitExceeded {
            limit: Limit::FunctionData,
            ..
        }) => {}
        other => panic!("{other:?}"),
    }
}

// --- type 3 (7.10.4) ---

/// Two exponentials stitched at 0.5: the first maps [0, 0.5) to its own
/// 0 to 1 and the second maps [0.5, 1] to its 1 down to 0.
fn stitched(bounds: &str, domain: &str) -> Vec<(u32, Vec<u8>)> {
    vec![
        (
            61,
            b"<< /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [10] /N 1 >>".to_vec(),
        ),
        (
            62,
            b"<< /FunctionType 2 /Domain [0 1] /C0 [20] /C1 [30] /N 1 >>".to_vec(),
        ),
        (
            60,
            format!(
                "<< /FunctionType 3 /Domain [{domain}] /Functions [61 0 R 62 0 R] \
                 /Bounds [{bounds}] /Encode [0 1 1 0] >>"
            )
            .into_bytes(),
        ),
    ]
}

fn stitching(bounds: &str, domain: &str) -> Function {
    let objects = stitched(bounds, domain);
    function_of(
        &objects
            .iter()
            .map(|(n, b)| (*n, b.as_slice()))
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

/// `x' = Interpolate(x, Bounds_{i-1}, Bounds_i, Encode_2i, Encode_2i+1)`.
#[test]
fn a_stitching_function_encodes_each_subdomain() {
    let f = stitching("0.5", "0 1");
    // x = 0.25 is halfway through the first subdomain: x' = 0.5, f1 = 5.
    near(f.call(&[0.25]).unwrap()[0], 5.0, 1e-12);
    // x = 0.75 is halfway through the second, encoded 1 down to 0: x' = 0.5.
    near(f.call(&[0.75]).unwrap()[0], 25.0, 1e-12);
    // The bound belongs to the second subdomain (half-open on the right):
    // x' = 1, so f2(1) = 30.
    near(f.call(&[0.5]).unwrap()[0], 30.0, 1e-12);
    // The last subdomain is closed: x = 1 is x' = 0, f2(0) = 20.
    near(f.call(&[1.0]).unwrap()[0], 20.0, 1e-12);
    near(f.call(&[0.0]).unwrap()[0], 0.0, 1e-12);
}

/// 7.10.4: when the last bound equals the top of the domain, a value
/// there is encoded as `Encode_2i`.
#[test]
fn a_subdomain_with_no_width_is_encoded_as_its_first_encode_value() {
    let f = stitching("1", "0 1");
    // x = 1 is in the second subdomain [1, 1]; Encode [1 0] gives 1.
    near(f.call(&[1.0]).unwrap()[0], 30.0, 1e-12);
}

#[test]
fn a_stitching_function_with_a_bad_entry_is_refused() {
    let with = |entries: &str| {
        let objects = [
            (61, b"<< /FunctionType 2 /Domain [0 1] /N 1 >>".to_vec()),
            (
                60,
                format!("<< /FunctionType 3 /Domain [0 1] {entries} >>").into_bytes(),
            ),
        ];
        function_of(
            &objects
                .iter()
                .map(|(n, b)| (*n, b.as_slice()))
                .collect::<Vec<_>>(),
        )
    };
    for (entries, key) in [
        ("/Functions [61 0 R] /Bounds [0.5] /Encode [0 1]", "Bounds"),
        ("/Functions [61 0 R] /Bounds [] /Encode [0 1 0 1]", "Encode"),
        ("/Functions [] /Bounds [] /Encode []", "Functions"),
        ("/Bounds [] /Encode [0 1]", "Functions"),
        (
            "/Functions [61 0 R 61 0 R] /Bounds [0.5] /Encode [0 1 0 1] /Range [0 1 0 1]",
            "Range",
        ),
    ] {
        match with(entries) {
            Err(PdfError::InvalidEntry { key: found, .. }) => assert_eq!(found, key, "{entries}"),
            other => panic!("{entries}: {other:?}"),
        }
    }
}

/// A stitching function that names itself nests without end; its depth is
/// limited.
#[test]
fn a_stitching_function_that_contains_itself_is_refused() {
    let body = b"<< /FunctionType 3 /Domain [0 1] /Functions [60 0 R] /Bounds [] /Encode [0 1] >>";
    match function_of(&[(60, body)]) {
        Err(PdfError::LimitExceeded {
            limit: Limit::FunctionNesting,
            ..
        }) => {}
        other => panic!("{other:?}"),
    }
}

// --- type 4 (7.10.5) ---

fn postscript(code: &str, inputs: usize, outputs: usize) -> Result<Function, PdfError> {
    let entries = format!(
        " /FunctionType 4 /Domain [{}] /Range [{}]",
        "-1000 1000 ".repeat(inputs),
        "-10000000000 10000000000 ".repeat(outputs)
    );
    function_of(&[(60, &stream(&entries, code.as_bytes()))])
}

fn run(code: &str, input: &[f64], outputs: usize) -> Vec<f64> {
    postscript(code, input.len(), outputs)
        .unwrap()
        .call(input)
        .unwrap()
}

/// 7.10.5.1: the example of the specification, a DoubleDot spot function.
#[test]
fn the_double_dot_example_computes() {
    let f = function_of(&[(
        60,
        &stream(
            " /FunctionType 4 /Domain [-1 1 -1 1] /Range [-1 1]",
            b"{ 360 mul sin 2 div exch 360 mul sin 2 div add }",
        ),
    )])
    .unwrap();
    // x = y = 0.25: sin(90 degrees) / 2 twice.
    near(f.call(&[0.25, 0.25]).unwrap()[0], 1.0, 1e-12);
    // x = 0.25, y = -0.25: 0.5 - 0.5.
    near(f.call(&[0.25, -0.25]).unwrap()[0], 0.0, 1e-12);
}

/// Table 42, the arithmetic operators, each with a result worked by hand.
#[test]
fn arithmetic_operators_compute() {
    let cases: &[(&str, f64)] = &[
        ("pop 2 3 add", 5.0),
        ("pop 7 2 sub", 5.0),
        ("pop 3 4 mul", 12.0),
        ("pop 7 2 div", 3.5),
        ("pop 7 2 idiv", 3.0),
        ("pop -7 2 idiv", -3.0),
        ("pop 7 3 mod", 1.0),
        ("pop -7 3 mod", -1.0),
        ("pop 3 neg", -3.0),
        ("pop -3 abs", 3.0),
        ("pop -2.5 abs", 2.5),
        ("pop 2.5 floor", 2.0),
        ("pop -2.5 floor", -3.0),
        ("pop 2.5 ceiling", 3.0),
        ("pop -2.5 ceiling", -2.0),
        ("pop 2.5 round", 3.0),
        ("pop -2.5 round", -2.0),
        ("pop 2.7 truncate", 2.0),
        ("pop -2.7 truncate", -2.0),
        ("pop -2.7 cvi", -2.0),
        ("pop 3 cvr", 3.0),
        ("pop 9 sqrt", 3.0),
        ("pop 100 log", 2.0),
        ("pop 1 ln", 0.0),
        ("pop 2 3 exp", 8.0),
        ("pop 0 sin", 0.0),
        ("pop 90 sin", 1.0),
        ("pop 0 cos", 1.0),
        ("pop 180 cos", -1.0),
        ("pop 1 1 atan", 45.0),
        ("pop 0 -1 atan", 180.0),
        ("pop -1 0 atan", 270.0),
        // Integer overflow gives a real, as in PostScript.
        ("pop 2147483647 1 add", 2147483648.0),
        ("pop 2147483647 neg 1 sub 1 sub", -2147483649.0),
    ];
    for (code, expected) in cases {
        let result = run(&format!("{{ {code} }}"), &[0.0], 1);
        near(result[0], *expected, 1e-9);
    }
}

#[test]
fn relational_boolean_and_bitwise_operators_compute() {
    let cases: &[(&str, f64)] = &[
        ("pop 1 2 lt { 10 } { 20 } ifelse", 10.0),
        ("pop 2 1 lt { 10 } { 20 } ifelse", 20.0),
        ("pop 2 2 le { 1 } { 0 } ifelse", 1.0),
        ("pop 2 2 ge { 1 } { 0 } ifelse", 1.0),
        ("pop 2 1 gt { 1 } { 0 } ifelse", 1.0),
        ("pop 2 2.0 eq { 1 } { 0 } ifelse", 1.0),
        ("pop 2 3 ne { 1 } { 0 } ifelse", 1.0),
        ("pop true { 5 } if", 5.0),
        ("pop false { 5 } if 6", 6.0),
        ("pop true false and { 1 } { 2 } ifelse", 2.0),
        ("pop true false or { 1 } { 2 } ifelse", 1.0),
        ("pop true true xor { 1 } { 2 } ifelse", 2.0),
        ("pop false not { 1 } { 2 } ifelse", 1.0),
        ("pop 5 3 and", 1.0),
        ("pop 5 3 or", 7.0),
        ("pop 5 3 xor", 6.0),
        ("pop 0 not", -1.0),
        ("pop 1 3 bitshift", 8.0),
        ("pop 8 -2 bitshift", 2.0),
        ("pop -1 -28 bitshift", 15.0),
        ("pop 1 40 bitshift", 0.0),
        ("pop true false eq { 1 } { 2 } ifelse", 2.0),
        ("pop 1 2 lt 3 4 gt or { 7 } { 8 } ifelse", 7.0),
        // Nested conditionals.
        ("pop true { false { 1 } { 2 } ifelse } { 3 } ifelse", 2.0),
    ];
    for (code, expected) in cases {
        let result = run(&format!("{{ {code} }}"), &[0.0], 1);
        near(result[0], *expected, 1e-9);
    }
}

#[test]
fn stack_operators_compute() {
    let cases: &[(&str, &[f64])] = &[
        ("pop 1 2 exch", &[2.0, 1.0]),
        ("pop 5 dup", &[5.0, 5.0]),
        ("pop 1 2 pop", &[1.0]),
        ("pop 1 2 3 3 1 roll", &[3.0, 1.0, 2.0]),
        ("pop 1 2 3 3 -1 roll", &[2.0, 3.0, 1.0]),
        ("pop 1 2 3 3 3 roll", &[1.0, 2.0, 3.0]),
        ("pop 1 2 3 3 4 roll", &[3.0, 1.0, 2.0]),
        ("pop 1 2 3 0 0 roll", &[1.0, 2.0, 3.0]),
        ("pop 1 2 3 2 copy", &[1.0, 2.0, 3.0, 2.0, 3.0]),
        ("pop 1 2 3 0 copy", &[1.0, 2.0, 3.0]),
        ("pop 1 2 3 2 index", &[1.0, 2.0, 3.0, 1.0]),
        ("pop 1 2 3 0 index", &[1.0, 2.0, 3.0, 3.0]),
    ];
    for (code, expected) in cases {
        let result = run(&format!("{{ {code} }}"), &[0.0], expected.len());
        assert_eq!(result, *expected, "{code}");
    }
}

/// The input is the initial stack, first input at the bottom.
#[test]
fn inputs_are_the_initial_stack() {
    assert_eq!(run("{ exch }", &[1.0, 2.0], 2), [2.0, 1.0]);
    assert_eq!(run("{ }", &[0.25, 0.5], 2), [0.25, 0.5]);
    // Comments and white space of every kind are skipped.
    assert_eq!(run("{ % comment\n\t1 add\r\n }", &[0.5], 1), [1.5]);
}

/// 7.10.5.2: the errors of a calculator function are reported.
#[test]
fn execution_errors_are_reported() {
    let cases: &[(&str, FunctionFault)] = &[
        ("{ pop pop 1 }", FunctionFault::StackUnderflow),
        ("{ add }", FunctionFault::StackUnderflow),
        ("{ 1 0 div }", FunctionFault::Undefined),
        ("{ 1 0 idiv }", FunctionFault::Undefined),
        ("{ 1 0 mod }", FunctionFault::Undefined),
        ("{ -1 sqrt }", FunctionFault::Range),
        ("{ 0 ln }", FunctionFault::Range),
        ("{ -1 log }", FunctionFault::Range),
        ("{ 0 0 atan }", FunctionFault::Undefined),
        ("{ -8 0.5 exp }", FunctionFault::Range),
        ("{ 0 -1 exp }", FunctionFault::Undefined),
        ("{ 1.5 not }", FunctionFault::Type),
        ("{ true 1 add }", FunctionFault::Type),
        ("{ 1 true and }", FunctionFault::Type),
        ("{ 1.5 2 idiv }", FunctionFault::Type),
        ("{ 1 { 2 } if }", FunctionFault::Type),
        ("{ true 1 eq }", FunctionFault::Type),
        ("{ pop true }", FunctionFault::Type),
        ("{ 1 -1 copy }", FunctionFault::Range),
        ("{ 1 5 copy }", FunctionFault::StackUnderflow),
        ("{ 1 5 index }", FunctionFault::StackUnderflow),
        ("{ 1 -1 index }", FunctionFault::Range),
        ("{ 1 2 3 5 1 roll }", FunctionFault::StackUnderflow),
        ("{ 1 2 }", FunctionFault::OutputCount { expected: 1, found: 3 }),
        ("{ pop }", FunctionFault::OutputCount { expected: 1, found: 0 }),
        ("{ 1 2 3 4 5 6 7 8 9 10 dup dup dup dup dup dup dup dup dup dup 20 copy 20 copy 20 copy 20 copy 20 copy 20 copy }", FunctionFault::StackOverflow),
    ];
    for (code, expected) in cases {
        let f = postscript(code, 1, 1).unwrap_or_else(|e| panic!("{code}: {e:?}"));
        assert_eq!(fault(f.call(&[0.0]).unwrap_err()), *expected, "{code}");
    }
    // A real too large for 32 bits cannot become an integer.
    let f = postscript("{ pop 3000000000.0 cvi }", 1, 1).unwrap();
    assert_eq!(fault(f.call(&[0.0]).unwrap_err()), FunctionFault::Range);
}

/// A result that is not a finite number is an error, not a value.
#[test]
fn a_result_that_overflows_is_undefined() {
    let f = postscript("{ pop 1e30 1 mul }", 1, 1);
    // Exponent notation is not PDF syntax, so the code is refused.
    assert!(matches!(f, Err(PdfError::InvalidEntry { .. })));
    let f = postscript(
        "{ pop 100000.0 dup mul dup mul dup mul dup mul dup mul dup mul dup mul dup mul \
         dup mul dup mul dup mul dup mul dup mul dup mul dup mul dup mul }",
        1,
        1,
    )
    .unwrap();
    assert_eq!(fault(f.call(&[0.0]).unwrap_err()), FunctionFault::Undefined);
}

/// The reader detects syntax errors (7.10.5.2).
#[test]
fn calculator_code_with_a_syntax_error_is_refused() {
    for code in [
        "1 2 add",
        "{ 1 2 add",
        "{ 1 2 add } extra",
        "{ foo }",
        "{ { 1 } }",
        "{ { 1 } foo }",
        "{ { 1 } { 2 } if }",
        "{ { 1 } { 2 } { 3 } ifelse }",
        "{ if }",
        "{ 1.2.3 }",
        "{ 1e3 }",
        "{ - }",
        "{ 16#FF }",
        "}",
        "",
    ] {
        match postscript(code, 1, 1) {
            Err(PdfError::InvalidEntry { key, .. }) => assert_eq!(key, "Function", "{code:?}"),
            other => panic!("{code:?}: {other:?}"),
        }
    }
}

/// A type 4 function needs both `Domain` and `Range`.
#[test]
fn a_calculator_function_needs_a_range() {
    let body = stream(" /FunctionType 4 /Domain [0 1]", b"{ }");
    match function_of(&[(60, &body)]) {
        Err(PdfError::InvalidEntry { key, .. }) => assert_eq!(key, "Range"),
        other => panic!("{other:?}"),
    }
}

/// Table C.1 sets 32 bits for an integer, and 7.10.5.1 a stack of at
/// least 100: a program may nest conditionals as deep as the reader's
/// bound on nesting.
#[test]
fn deeply_nested_conditionals_are_refused_not_overrun() {
    let depth = 5000;
    let code = format!(
        "{}{}{}",
        "{ true ".repeat(depth),
        "1",
        " } if".repeat(depth)
    );
    let code = format!("{{ pop {code} }}");
    assert!(postscript(&code, 1, 1).is_err());
}
