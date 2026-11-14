// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Reading `Filter` and `DecodeParms` (7.3.8.2, Table 5) and the parameters
//! of Table 8, and running a pipeline with no filter.

use std::borrow::Cow;

use kernkraft_pdf::filter::{Filter, FilterKind, Pipeline, Predictor, PredictorFunction};
use kernkraft_pdf::syntax::limits::MAX_FILTERS;
use kernkraft_pdf::{Diagnostics, Limit, PdfError};

use crate::{
    assert_parameters, assert_too_large, clean, decode, decode_capped, pipeline, stored, OFFSET,
};

const STREAM: &str = "the stream";

fn filters(dictionary: &str) -> Vec<Filter> {
    pipeline(dictionary).unwrap().filters().to_vec()
}

/// The predictor of the pipeline's only filter, as (function, colors, bits,
/// columns).
fn predictor(dictionary: &str) -> (PredictorFunction, u32, u8, u32) {
    match filters(dictionary).as_slice() {
        [Filter::Flate { predictor } | Filter::Lzw { predictor, .. }] => (
            predictor.function(),
            predictor.colors(),
            predictor.bits_per_component(),
            predictor.columns(),
        ),
        other => panic!("not one filter with a predictor: {other:?}"),
    }
}

fn plain_lzw() -> Filter {
    Filter::Lzw {
        early_change: true,
        predictor: Predictor::default(),
    }
}

#[test]
fn no_filter_is_an_empty_pipeline() {
    for dictionary in [
        "<< >>",
        "<</Filter null>>",
        "<</Filter []>>",
        "<</Length 5>>",
    ] {
        assert_eq!(filters(dictionary), [], "{dictionary}");
        assert_eq!(pipeline(dictionary).unwrap(), Pipeline::default());
    }
}

#[test]
fn one_name() {
    assert_eq!(filters("<</Filter /ASCIIHexDecode>>"), [Filter::AsciiHex]);
}

#[test]
fn array_of_example_2() {
    assert_eq!(
        filters("<</Filter [ /ASCII85Decode /LZWDecode ]>>"),
        [Filter::Ascii85, plain_lzw()]
    );
}

#[test]
fn all_five_names() {
    let kinds = [
        ("ASCIIHexDecode", FilterKind::AsciiHex),
        ("ASCII85Decode", FilterKind::Ascii85),
        ("LZWDecode", FilterKind::Lzw),
        ("FlateDecode", FilterKind::Flate),
        ("RunLengthDecode", FilterKind::RunLength),
    ];
    for (name, kind) in kinds {
        let found = filters(&format!("<</Filter /{name}>>"));
        assert_eq!(found.iter().map(Filter::kind).collect::<Vec<_>>(), [kind]);
    }
}

#[test]
fn names_not_decoded_here() {
    for name in ["DCTDecode", "Fl", "Bogus", "flatedecode", "FlateDecodeX"] {
        match pipeline(&format!("<</Filter [/ASCIIHexDecode /{name}]>>")) {
            Err(PdfError::UnknownFilter { name: found }) => assert_eq!(found, name),
            other => panic!("expected UnknownFilter {name}, got {other:?}"),
        }
    }
}

#[test]
fn filter_of_the_wrong_type() {
    for dictionary in [
        "<</Filter 3>>",
        "<</Filter (FlateDecode)>>",
        "<</Filter [/FlateDecode 3]>>",
        "<</Filter 1 0 R>>",
        "<</Filter [1 0 R]>>",
        "<</Filter [[/FlateDecode]]>>",
    ] {
        assert_parameters(pipeline(dictionary), STREAM, "Filter");
    }
}

#[test]
fn parms_dictionary_with_one_filter() {
    let expected = (PredictorFunction::Png, 1, 8, 4);
    assert_eq!(
        predictor("<</Filter /FlateDecode /DecodeParms <</Predictor 12 /Columns 4>>>>"),
        expected
    );
    assert_eq!(
        predictor("<</Filter [/FlateDecode] /DecodeParms <</Predictor 12 /Columns 4>>>>"),
        expected
    );
    assert_eq!(
        predictor("<</Filter [/FlateDecode] /DecodeParms [<</Predictor 12 /Columns 4>>]>>"),
        expected
    );
}

#[test]
fn parms_array_matches_filters() {
    let found = filters(
        "<</Filter [/ASCII85Decode /FlateDecode] /DecodeParms [null <</Predictor 2 /Colors 3>>]>>",
    );
    assert_eq!(found[0], Filter::Ascii85);
    match &found[1] {
        Filter::Flate { predictor } => assert_eq!(
            (
                predictor.function(),
                predictor.colors(),
                predictor.bits_per_component(),
                predictor.columns()
            ),
            (PredictorFunction::Tiff, 3, 8, 1)
        ),
        other => panic!("{other:?}"),
    }
    assert_eq!(found.len(), 2);
}

#[test]
fn parms_shape_refused() {
    for dictionary in [
        "<</Filter [/ASCII85Decode /FlateDecode] /DecodeParms <</Predictor 2>>>>",
        "<</Filter [/ASCII85Decode /FlateDecode] /DecodeParms [null]>>",
        "<</Filter /FlateDecode /DecodeParms [null null]>>",
        "<</Filter /FlateDecode /DecodeParms [5]>>",
        "<</Filter /FlateDecode /DecodeParms 5>>",
        "<</Filter /FlateDecode /DecodeParms 1 0 R>>",
        "<</Filter /FlateDecode /DecodeParms [1 0 R]>>",
        "<</DecodeParms <</Predictor 2>>>>",
        "<</DecodeParms [null]>>",
    ] {
        assert_parameters(pipeline(dictionary), STREAM, "DecodeParms");
    }
}

#[test]
fn parms_without_a_filter_may_be_null_or_empty() {
    assert_eq!(filters("<</DecodeParms null>>"), []);
    assert_eq!(filters("<</DecodeParms []>>"), []);
}

#[test]
fn parms_of_a_filter_that_takes_none() {
    for name in ["ASCIIHexDecode", "ASCII85Decode", "RunLengthDecode"] {
        let found = filters(&format!(
            "<</Filter /{name} /DecodeParms <</Predictor 99 /EarlyChange /No>>>>"
        ));
        assert_eq!(found.len(), 1);
    }
}

#[test]
fn table_8_defaults() {
    assert_eq!(filters("<</Filter /LZWDecode>>"), [plain_lzw()]);
    assert_eq!(
        filters("<</Filter /LZWDecode /DecodeParms << >>>>"),
        [plain_lzw()]
    );
    assert_eq!(
        predictor("<</Filter /FlateDecode>>"),
        (PredictorFunction::None, 1, 8, 1)
    );
    // An entry whose value is null is an entry that is absent (7.3.7).
    assert_eq!(
        predictor("<</Filter /FlateDecode /DecodeParms <</Predictor null /Columns null>>>>"),
        (PredictorFunction::None, 1, 8, 1)
    );
}

#[test]
fn early_change() {
    let early = |value: &str| {
        pipeline(&format!(
            "<</Filter /LZWDecode /DecodeParms <</EarlyChange {value}>>>>"
        ))
        .map(|pipeline| match pipeline.filters() {
            [Filter::Lzw { early_change, .. }] => *early_change,
            other => panic!("{other:?}"),
        })
    };
    assert!(!early("0").unwrap());
    assert!(early("1").unwrap());
    for value in ["2", "-1", "1.0", "/A", "true"] {
        assert_parameters(early(value), "LZWDecode", "EarlyChange");
    }
    // Flate has no such parameter and does not read it.
    assert_eq!(
        predictor("<</Filter /FlateDecode /DecodeParms <</EarlyChange 7>>>>"),
        (PredictorFunction::None, 1, 8, 1)
    );
}

#[test]
fn predictor_values() {
    let function = |value: &str| {
        pipeline(&format!(
            "<</Filter /FlateDecode /DecodeParms <</Predictor {value}>>>>"
        ))
        .map(|pipeline| match pipeline.filters() {
            [Filter::Flate { predictor }] => predictor.function(),
            other => panic!("{other:?}"),
        })
    };
    assert_eq!(function("1").unwrap(), PredictorFunction::None);
    assert_eq!(function("2").unwrap(), PredictorFunction::Tiff);
    for value in 10..=15 {
        assert_eq!(
            function(&value.to_string()).unwrap(),
            PredictorFunction::Png
        );
    }
    for value in [
        "0",
        "3",
        "9",
        "16",
        "-1",
        "2.0",
        "/Up",
        "9223372036854775807",
    ] {
        assert_parameters(function(value), "FlateDecode", "Predictor");
    }
}

#[test]
fn colors_bits_columns_ranges() {
    let with = |filter: &str, key: &str, value: &str| {
        pipeline(&format!(
            "<</Filter /{filter} /DecodeParms <</{key} {value}>>>>"
        ))
    };
    for (key, bad) in [
        ("Colors", &["0", "-1", "4294967296", "2.0", "(3)"][..]),
        ("Columns", &["0", "-1", "4294967296", "2.0", "(3)"][..]),
        (
            "BitsPerComponent",
            &["0", "3", "5", "32", "-8", "8.0", "256"][..],
        ),
    ] {
        for value in bad {
            assert_parameters(with("FlateDecode", key, value), "FlateDecode", key);
            assert_parameters(with("LZWDecode", key, value), "LZWDecode", key);
        }
    }
    for bits in [1, 2, 4, 8, 16] {
        let dictionary =
            format!("<</Filter /FlateDecode /DecodeParms <</BitsPerComponent {bits}>>>>");
        assert_eq!(
            predictor(&dictionary),
            (PredictorFunction::None, 1, bits, 1)
        );
    }
    assert_eq!(
        predictor("<</Filter /LZWDecode /DecodeParms <</Colors 4294967295 /Columns 1>>>>"),
        (PredictorFunction::None, 4_294_967_295, 8, 1)
    );
}

#[test]
fn row_too_large_for_the_arithmetic() {
    // 4294967295 * 4294967295 * 16 bits is about 2^65 bytes: no usize
    // holds it.
    assert_parameters(
        pipeline(
            "<</Filter /FlateDecode /DecodeParms \
             <</Predictor 12 /Colors 4294967295 /Columns 4294967295 /BitsPerComponent 16>>>>",
        ),
        "FlateDecode",
        "Columns",
    );
}

/// A row of 4 GiB is a valid parameter where a usize holds it, and costs
/// nothing until data arrives: no row is allocated ahead of the data.
#[cfg(target_pointer_width = "64")]
#[test]
fn a_huge_row_allocates_nothing() {
    let dictionary = "<</Filter /FlateDecode /DecodeParms <</Predictor 12 /Columns 4294967295>>>>";
    assert_eq!(decode(dictionary, &stored(b"")), clean(b""));
    let dictionary = "<</Filter /FlateDecode /DecodeParms <</Predictor 2 /Columns 4294967295>>>>";
    assert_eq!(decode(dictionary, &stored(b"")), clean(b""));
}

#[test]
fn filter_count_at_the_cap() {
    let chain = |count: usize| format!("<</Filter [{}]>>", "/ASCIIHexDecode ".repeat(count));
    assert_eq!(filters(&chain(MAX_FILTERS)).len(), MAX_FILTERS);
    let text = chain(MAX_FILTERS + 1);
    match pipeline(&text) {
        Err(PdfError::LimitExceeded { offset, limit }) => {
            assert_eq!(limit, Limit::FilterCount);
            assert_eq!(offset, text.find('[').unwrap() as u64);
        }
        other => panic!("expected LimitExceeded, got {other:?}"),
    }
}

#[test]
fn empty_pipeline_borrows() {
    let pipeline = pipeline("<< >>").unwrap();
    let mut diagnostics = Diagnostics::new();
    let decoded = pipeline
        .decode(b"abc", 3, OFFSET, &mut diagnostics)
        .unwrap();
    assert!(matches!(decoded.data, Cow::Borrowed(b"abc")));
    assert!(decoded.complete);
    assert!(diagnostics.is_empty());
    assert_too_large(pipeline.decode(b"abc", 2, OFFSET, &mut diagnostics), 2);
    assert_eq!(decode_capped("<< >>", b"", 0).unwrap(), clean(b""));
}

#[test]
fn a_chain_decodes_in_order() {
    // Hex of the hex digits of `Hi`: 48 69 -> `4869` -> 34 38 36 39.
    assert_eq!(
        decode(
            "<</Filter [/ASCIIHexDecode /ASCIIHexDecode]>>",
            b"34383639>"
        ),
        // The inner data `4869` has no marker of its own.
        crate::unmarked(b"Hi", FilterKind::AsciiHex)
    );
}

#[test]
fn a_stage_after_damage_still_runs() {
    // The first stage stops at `X` having decoded `4869`, which the second
    // decodes whole.
    let run = decode(
        "<</Filter [/ASCIIHexDecode /ASCIIHexDecode]>>",
        b"34383639X",
    );
    assert_eq!(run.data, b"Hi");
    assert!(!run.complete);
    assert_eq!(
        run.kinds,
        [
            kernkraft_pdf::DiagnosticKind::FilterDataDamaged {
                filter: FilterKind::AsciiHex
            },
            kernkraft_pdf::DiagnosticKind::MissingEndOfData {
                filter: FilterKind::AsciiHex
            },
        ]
    );
}
