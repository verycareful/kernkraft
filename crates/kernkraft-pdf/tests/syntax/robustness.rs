// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Deterministic mutation of syntax fixtures: every truncation, every
//! substitution of one byte by a byte that means something to the lexer, and
//! every splice of one fixture's start onto another. Each mutated input must
//! come back as an object or a typed syntax error; a panic fails the test.

use kernkraft_pdf::syntax::Parser;
use kernkraft_pdf::{Diagnostics, PdfError};

use crate::memory;

/// Fixtures that hold one direct object.
pub const OBJECT_FIXTURES: &[&[u8]] = &[
    b"[549 3.14 false (Ralph) /SomeName]",
    b"<< /Type /Example /Subtype /DictionaryExample /Version 0.01 /IntegerItem 12
       /StringItem (a string) /Subdictionary << /Item1 0.4 /Item2 true
       /LastItem (not!) /VeryLastItem (OK) >> >>",
    b"[/Name1 /A;Name_With-Various***Characters? /1.2 /$$ /@pattern /.notdef
      /lime#20Green /paired#28#29parentheses /The_Key_of_F#23_Minor /A#42 /]",
    b"[() (a (b) c) (\\n\\r\\t\\b\\f\\(\\)\\\\) (\\0053 \\53 \\245) (These \\\ntwo) (a\r\nb)
      <4E6F762073686D6F7A206B6120706F702E> <901FA> < 90 1f A3 > 34.5 -.002 +17 4.]",
    b"<</A <</B <</C <</D <</E <</F <</G <</H [1 0 R 2 0 R null 3 65535 R]>>>>>>>>>>>>>>>>
      % a comment before the end",
];

/// Fixtures that hold an indirect object.
const INDIRECT_FIXTURES: &[&[u8]] = &[
    b"12 0 obj\n(Brillig)\nendobj\n",
    b"3 0 obj\n<</Length 4 /Filter [/ASCIIHexDecode]>>\nstream\r\nDATA\nendstream\nendobj\n",
];

/// Bytes that change how the lexer reads what follows: every delimiter, the
/// two escape characters, white space of each kind, a digit at each end of
/// the range, the reference keyword, and the two extreme byte values.
const REPLACEMENTS: &[u8] = b"()<>[]{}/%#\\\x00\n\r 09R\xFF";

fn fixtures() -> Vec<&'static [u8]> {
    OBJECT_FIXTURES
        .iter()
        .chain(INDIRECT_FIXTURES)
        .copied()
        .collect()
}

/// Runs both entry points over `input` and returns their errors.
fn errors(input: &[u8]) -> Vec<PdfError> {
    let source = memory(input);
    let mut errors = Vec::new();
    let mut diagnostics = Diagnostics::new();
    if let Err(error) = Parser::new(&source).parse_object(&mut diagnostics) {
        errors.push(error);
    }
    if let Err(error) = Parser::new(&source).parse_indirect_object(&mut diagnostics) {
        errors.push(error);
    }
    errors
}

fn assert_syntax_errors(input: &[u8]) {
    for error in errors(input) {
        assert!(
            matches!(
                error,
                PdfError::UnexpectedEnd { .. }
                    | PdfError::UnexpectedToken { .. }
                    | PdfError::LimitExceeded { .. }
            ),
            "{error:?} for {:?}",
            String::from_utf8_lossy(input)
        );
    }
}

#[test]
fn the_fixtures_parse_whole() {
    for fixture in OBJECT_FIXTURES {
        let source = memory(fixture);
        let result = Parser::new(&source).parse_object(&mut Diagnostics::new());
        assert!(result.is_ok(), "{result:?}");
    }
    for fixture in INDIRECT_FIXTURES {
        let source = memory(fixture);
        let result = Parser::new(&source).parse_indirect_object(&mut Diagnostics::new());
        assert!(result.is_ok(), "{result:?}");
    }
}

#[test]
fn every_truncation_returns_an_object_or_a_syntax_error() {
    for fixture in fixtures() {
        for length in 0..fixture.len() {
            assert_syntax_errors(&fixture[..length]);
        }
    }
}

#[test]
fn every_single_byte_substitution_returns_an_object_or_a_syntax_error() {
    for fixture in fixtures() {
        let mut mutated = fixture.to_vec();
        for offset in 0..fixture.len() {
            for &replacement in REPLACEMENTS {
                mutated[offset] = replacement;
                assert_syntax_errors(&mutated);
            }
            mutated[offset] = fixture[offset];
        }
    }
}

#[test]
fn every_splice_returns_an_object_or_a_syntax_error() {
    for first in fixtures() {
        for second in fixtures() {
            for cut in 0..first.len() {
                let mut spliced = first[..cut].to_vec();
                spliced.extend_from_slice(second);
                assert_syntax_errors(&spliced);
            }
        }
    }
}
