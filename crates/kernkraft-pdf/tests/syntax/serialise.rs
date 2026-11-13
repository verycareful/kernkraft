// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Serialiser tests: the canonical form of each object, and reading back
//! what was written.

use kernkraft_pdf::syntax::limits::{MAX_NESTING_DEPTH, PORTABLE_NAME_BYTES};
use kernkraft_pdf::syntax::{
    write_indirect_object, write_object, Name, Object, ObjectId, Parser, Value,
};
use kernkraft_pdf::{Diagnostics, Limit};

use crate::robustness::OBJECT_FIXTURES;
use crate::{array, assert_limit, boolean, dict, int, memory, name, null, real, reference, string};

fn written(object: &Object) -> Vec<u8> {
    let mut out = Vec::new();
    write_object(object, &mut out).unwrap();
    out
}

fn text(object: &Object) -> String {
    String::from_utf8(written(object)).unwrap()
}

fn parsed(input: &[u8]) -> (Object, Diagnostics) {
    let source = memory(input);
    let mut diagnostics = Diagnostics::new();
    let object = Parser::new(&source).parse_object(&mut diagnostics).unwrap();
    (object, diagnostics)
}

/// `depth` arrays, one inside the other.
fn nested_arrays(depth: usize) -> Object {
    let mut object = array(vec![]);
    for _ in 1..depth {
        object = array(vec![object]);
    }
    object
}

#[test]
fn scalars_in_canonical_form() {
    assert_eq!(text(&null()), "null");
    assert_eq!(text(&boolean(true)), "true");
    assert_eq!(text(&boolean(false)), "false");
    assert_eq!(text(&int(-98)), "-98");
    assert_eq!(text(&int(0)), "0");
    assert_eq!(text(&int(i64::MIN)), i64::MIN.to_string());
    assert_eq!(text(&reference(12, 0)), "12 0 R");
}

#[test]
fn reals() {
    assert_eq!(text(&real(34.5)), "34.5");
    assert_eq!(text(&real(-0.002)), "-0.002");
    assert_eq!(text(&real(4.0)), "4.0");
    assert_eq!(text(&real(0.0)), "0.0");
    assert_eq!(text(&real(1e21)), format!("1{}.0", "0".repeat(21)));
    assert_eq!(text(&real(1e-7)), format!("0.{}1", "0".repeat(6)));
}

#[test]
fn string_escapes() {
    assert_eq!(written(&string(br"a(b)\c")), br"(a\(b\)\\c)");
    assert_eq!(
        written(&string(&[0x0D, 0x0A, 0x00, 0xFF])),
        [b'(', b'\\', b'r', 0x0A, 0x00, 0xFF, b')']
    );
    assert_eq!(text(&string(b"")), "()");
}

#[test]
fn name_escapes() {
    assert_eq!(text(&name("Name1")), "/Name1");
    assert_eq!(text(&name("lime Green")), "/lime#20Green");
    assert_eq!(
        text(&name("paired()parentheses")),
        "/paired#28#29parentheses"
    );
    assert_eq!(text(&name("The_Key_of_F#_Minor")), "/The_Key_of_F#23_Minor");
    assert_eq!(text(&name("")), "/");
    let bytes = Object::new(Value::Name(Name::new([0x00, 0x80, 0xFF])));
    assert_eq!(text(&bytes), "/#00#80#FF");
    assert_eq!(text(&name("A/B%C")), "/A#2FB#25C");
}

// 3.14 is the number the example of 7.3.6 holds, not an approximation of pi.
#[allow(clippy::approx_constant)]
#[test]
fn containers() {
    let example = array(vec![
        int(549),
        real(3.14),
        boolean(false),
        string(b"Ralph"),
        name("SomeName"),
    ]);
    assert_eq!(text(&example), "[549 3.14 false (Ralph) /SomeName]");
    assert_eq!(text(&array(vec![])), "[]");
    assert_eq!(
        text(&dict(vec![
            ("A", int(1)),
            ("B", array(vec![int(2), int(3)]))
        ])),
        "<< /A 1 /B [2 3] >>"
    );
    assert_eq!(text(&dict(vec![])), "<< >>");
}

#[test]
fn indirect_object() {
    let mut out = Vec::new();
    let id = ObjectId {
        number: 12,
        generation: 0,
    };
    write_indirect_object(id, &string(b"Brillig"), &mut out).unwrap();
    assert_eq!(out, b"12 0 obj\n(Brillig)\nendobj\n");

    let source = memory(&out);
    let mut diagnostics = Diagnostics::new();
    let read = Parser::new(&source)
        .parse_indirect_object(&mut diagnostics)
        .unwrap();
    assert_eq!((read.id, read.object), (id, string(b"Brillig")));
    assert!(diagnostics.is_empty());
}

#[test]
fn every_byte_survives_a_string_and_a_name() {
    let all: Vec<u8> = (0..=u8::MAX).collect();
    let original = string(&all);
    let (read, diagnostics) = parsed(&written(&original));
    assert_eq!(read, original);
    assert!(diagnostics.is_empty());

    // Zero is not a name byte (7.3.5), and a name longer than the portable
    // length raises a diagnostic, so the other 255 values go in pieces.
    for piece in all[1..].chunks(PORTABLE_NAME_BYTES) {
        let original = Object::new(Value::Name(Name::new(piece)));
        let (read, diagnostics) = parsed(&written(&original));
        assert_eq!(read, original);
        assert!(diagnostics.is_empty(), "{:?}", diagnostics.as_slice());
    }
}

#[test]
fn round_trip_of_built_objects() {
    let tree = dict(vec![
        ("Null", null()),
        ("Flags", array(vec![boolean(true), boolean(false)])),
        (
            "Integers",
            array(vec![int(0), int(i64::MIN), int(i64::MAX)]),
        ),
        (
            "Reals",
            array(vec![real(0.1), real(-1e-10), real(f64::MAX), real(4.0)]),
        ),
        (
            "Strings",
            array(vec![string(b""), string(b"(\\)\r\n"), string(&[0, 255])]),
        ),
        ("A Name", name("with space")),
        ("Reference", reference(u32::MAX, u16::MAX)),
        (
            "Nested",
            dict(vec![(
                "Inner",
                array(vec![dict(vec![("Deep", int(1))]), array(vec![])]),
            )]),
        ),
    ]);
    let first = written(&tree);
    let (read, _) = parsed(&first);
    assert_eq!(read, tree);
    assert_eq!(written(&read), first);
}

#[test]
fn round_trip_of_parsed_text() {
    for fixture in OBJECT_FIXTURES {
        let (first, _) = parsed(fixture);
        let (second, _) = parsed(&written(&first));
        assert_eq!(second, first, "{}", String::from_utf8_lossy(fixture));
    }
}

#[test]
fn nesting_at_the_cap() {
    let at_cap = written(&nested_arrays(MAX_NESTING_DEPTH));
    let mut expected = vec![b'['; MAX_NESTING_DEPTH];
    expected.extend(vec![b']'; MAX_NESTING_DEPTH]);
    assert_eq!(at_cap, expected);

    let mut out = b"x ".to_vec();
    let result = write_object(&nested_arrays(MAX_NESTING_DEPTH + 1), &mut out);
    // The cap is passed when the innermost array would be opened.
    assert_limit(
        result,
        (b"x ".len() + MAX_NESTING_DEPTH) as u64,
        Limit::NestingDepth,
    );
}

#[test]
fn output_is_appended() {
    let mut out = b"x ".to_vec();
    write_object(&int(7), &mut out).unwrap();
    assert_eq!(out, b"x 7");
}
