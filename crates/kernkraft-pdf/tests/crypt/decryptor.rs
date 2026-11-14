// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! What a decryptor does to objects and streams a test hands it.

use super::*;
use kernkraft_pdf::file::{Location, Stream};
use kernkraft_pdf::syntax::{Name, PdfString, Span};
use kernkraft_pdf::DiagnosticKind;

const ID: ObjectId = ObjectId {
    number: 40,
    generation: 0,
};
const SPAN: Span = Span {
    start: 100,
    end: 200,
};

fn dictionary(entries: &[(&str, Value)]) -> Dictionary {
    let mut dictionary = Dictionary::new();
    for (key, value) in entries {
        dictionary.insert(Name::new(key.as_bytes()), Object::new(value.clone()));
    }
    dictionary
}

fn name(text: &str) -> Value {
    Value::Name(Name::new(text.as_bytes()))
}

fn string_value(bytes: &[u8]) -> Value {
    Value::String(PdfString::new(bytes))
}

fn file_object(body: Body, location: Location) -> FileObject {
    FileObject {
        id: ID,
        body,
        location,
    }
}

/// The value of `key` in the dictionary `object` holds.
fn entry(object: &FileObject, key: &[u8]) -> Value {
    let dictionary = match &object.body {
        Body::Object(object) => match object.value() {
            Value::Dictionary(dictionary) => dictionary,
            other => panic!("{other:?}"),
        },
        Body::Stream(stream) => &stream.dictionary,
    };
    get(dictionary, key).clone()
}

const SECRET: &[u8] = b"0123456789abcdefXYZ";

/// RC4 is its own inverse, so decrypting twice gives the bytes back; AES
/// strings of a test are not framed, which is reported.
#[test]
fn strings_of_an_object_from_the_file_are_decrypted() {
    let opened = unlock("rc4-128", USER).unwrap();
    let mut diagnostics = Diagnostics::new();
    let body = Value::Array(vec![
        Object::new(string_value(SECRET)),
        Object::new(Value::Integer(7)),
        Object::new(Value::Dictionary(dictionary(&[(
            "Nested",
            string_value(SECRET),
        )]))),
    ]);
    let mut object = file_object(Body::Object(Object::new(body)), Location::File(SPAN));
    opened
        .decryptor
        .decrypt_object(&mut object, &mut diagnostics)
        .unwrap();
    let Body::Object(decrypted) = &object.body else {
        panic!()
    };
    let Value::Array(items) = decrypted.value() else {
        panic!()
    };
    let once = string(items[0].value());
    assert_ne!(once, SECRET);
    assert_eq!(once.len(), SECRET.len());
    assert_eq!(items[1].value(), &Value::Integer(7));
    let Value::Dictionary(nested) = items[2].value() else {
        panic!()
    };
    assert_eq!(string(get(nested, b"Nested")), once);
    // The whole object is still where it was read from.
    assert_eq!(object.location, Location::File(SPAN));

    opened
        .decryptor
        .decrypt_object(&mut object, &mut diagnostics)
        .unwrap();
    let Body::Object(twice) = &object.body else {
        panic!()
    };
    let Value::Array(items) = twice.value() else {
        panic!()
    };
    assert_eq!(string(items[0].value()), SECRET);
    assert!(diagnostics.is_empty());
}

/// A string read from the source keeps its span until it is decrypted.
#[test]
fn a_decrypted_string_loses_its_span() {
    let mut opened = unlock("rc4-128", USER).unwrap();
    let info = reference(get(&opened.file.trailer().clone(), b"Info"));
    let raw = opened
        .file
        .load(info, &mut Diagnostics::new())
        .unwrap()
        .unwrap();
    let Body::Object(raw_info) = &raw.body else {
        panic!()
    };
    assert!(raw_info.span().is_some());
    let decrypted = opened.object(info);
    let Body::Object(info_object) = &decrypted.body else {
        panic!()
    };
    assert!(info_object.span().is_none());
    let Value::Dictionary(entries) = info_object.value() else {
        panic!()
    };
    for (_, value) in entries.iter() {
        assert!(value.span().is_none());
    }
    assert_eq!(decrypted.location, raw.location);
}

#[test]
fn objects_left_alone() {
    let opened = unlock("rc4-128", USER).unwrap();
    let mut diagnostics = Diagnostics::new();
    let plain = || {
        Object::new(Value::Dictionary(dictionary(&[(
            "S",
            string_value(SECRET),
        )])))
    };

    // Read from an object stream: decrypted with its stream.
    let mut object = file_object(
        Body::Object(plain()),
        Location::ObjectStream {
            stream: 3,
            index: 0,
        },
    );
    opened
        .decryptor
        .decrypt_object(&mut object, &mut diagnostics)
        .unwrap();
    assert_eq!(string(&entry(&object, b"S")), SECRET);

    // The dictionary of a cross-reference stream.
    let xref = dictionary(&[("Type", name("XRef")), ("S", string_value(SECRET))]);
    let mut object = file_object(
        Body::Stream(Stream {
            dictionary: xref,
            data: SPAN,
        }),
        Location::File(SPAN),
    );
    opened
        .decryptor
        .decrypt_object(&mut object, &mut diagnostics)
        .unwrap();
    assert_eq!(string(&entry(&object, b"S")), SECRET);

    // The encryption dictionary.
    let encrypt = reference(get(opened.file.trailer(), b"Encrypt"));
    let mut object = FileObject {
        id: encrypt,
        body: Body::Object(plain()),
        location: Location::File(SPAN),
    };
    opened
        .decryptor
        .decrypt_object(&mut object, &mut diagnostics)
        .unwrap();
    assert_eq!(string(&entry(&object, b"S")), SECRET);
    assert!(diagnostics.is_empty());
}

/// The `Contents` of a signature dictionary is not encrypted; its other
/// strings are.
#[test]
fn signature_contents_are_left_alone() {
    let opened = unlock("rc4-128", USER).unwrap();
    let signatures = [
        vec![("Type", name("Sig"))],
        vec![("Type", name("DocTimeStamp"))],
        vec![(
            "ByteRange",
            Value::Array(vec![Object::new(Value::Integer(0))]),
        )],
    ];
    for marks in signatures {
        let mut entries = marks.clone();
        entries.push(("Contents", string_value(SECRET)));
        entries.push(("Name", string_value(SECRET)));
        let mut object = file_object(
            Body::Object(Object::new(Value::Dictionary(dictionary(&entries)))),
            Location::File(SPAN),
        );
        opened
            .decryptor
            .decrypt_object(&mut object, &mut Diagnostics::new())
            .unwrap();
        assert_eq!(string(&entry(&object, b"Contents")), SECRET, "{marks:?}");
        assert_ne!(string(&entry(&object, b"Name")), SECRET, "{marks:?}");
    }
    // `Contents` elsewhere is encrypted like any string.
    let mut object = file_object(
        Body::Object(Object::new(Value::Dictionary(dictionary(&[(
            "Contents",
            string_value(SECRET),
        )])))),
        Location::File(SPAN),
    );
    opened
        .decryptor
        .decrypt_object(&mut object, &mut Diagnostics::new())
        .unwrap();
    assert_ne!(string(&entry(&object, b"Contents")), SECRET);
}

/// AES data that is not a vector and padded whole blocks is reported, and
/// stamped with the object.
#[test]
fn unframed_aes_data_is_reported() {
    for name in ["aes-128", "aes-256"] {
        let opened = unlock(name, USER).unwrap();
        for data in [&b""[..], b"short", &[0u8; 16], &[0u8; 40]] {
            let mut diagnostics = Diagnostics::new();
            opened
                .decryptor
                .decrypt_stream(ID, &Dictionary::new(), data, 77, &mut diagnostics)
                .unwrap();
            let found: Vec<_> = diagnostics
                .as_slice()
                .iter()
                .map(|d| (d.kind.clone(), d.object, d.offset))
                .collect();
            assert_eq!(
                found,
                [(DiagnosticKind::EncryptedDataDamaged, Some(ID), 77)],
                "{name}, {} bytes",
                data.len()
            );
        }
    }
}

/// A stream names its own crypt filter in its `Filter` (7.4.10).
#[test]
fn crypt_filters_named_by_a_stream() {
    let opened = unlock("aes-128", USER).unwrap();
    let decrypt = |dictionary: &Dictionary| {
        opened
            .decryptor
            .decrypt_stream(ID, dictionary, &[0u8; 32], 0, &mut Diagnostics::new())
    };
    let crypt = |parms: Option<Value>| {
        let mut entries = vec![("Filter", Value::Array(vec![Object::new(name("Crypt"))]))];
        if let Some(parms) = parms {
            entries.push(("DecodeParms", Value::Array(vec![Object::new(parms)])));
        }
        dictionary(&entries)
    };
    let parms = |filter: &str| Value::Dictionary(dictionary(&[("Name", name(filter))]));

    // Identity, by name or by default (Table 14), leaves the data alone.
    for dictionary in [crypt(None), crypt(Some(parms("Identity")))] {
        assert_eq!(&*decrypt(&dictionary).unwrap(), &[0u8; 32]);
    }
    // StdCF decrypts as the default does.
    let named = decrypt(&crypt(Some(parms("StdCF")))).unwrap().into_owned();
    let default = decrypt(&Dictionary::new()).unwrap().into_owned();
    assert_eq!(named, default);
    assert_ne!(named, [0u8; 32]);
    // A filter the file does not define.
    assert!(matches!(
        decrypt(&crypt(Some(parms("Missing")))),
        Err(PdfError::UnsupportedCryptFilter { name }) if name == "Missing"
    ));
    // A Crypt filter that is not the first is refused (7.4.10).
    let late = dictionary(&[(
        "Filter",
        Value::Array(vec![
            Object::new(name("ASCIIHexDecode")),
            Object::new(name("Crypt")),
        ]),
    )]);
    assert!(Pipeline::from_dictionary(&late).is_err());
    // A cross-reference stream is never encrypted.
    let xref = dictionary(&[("Type", name("XRef"))]);
    assert_eq!(&*decrypt(&xref).unwrap(), &[0u8; 32]);
}

/// A Crypt filter in a pipeline passes the data on as it is.
#[test]
fn the_pipeline_passes_crypt_data_on() {
    let crypt_then_hex = dictionary(&[(
        "Filter",
        Value::Array(vec![
            Object::new(name("Crypt")),
            Object::new(name("ASCIIHexDecode")),
        ]),
    )]);
    let pipeline = Pipeline::from_dictionary(&crypt_then_hex).unwrap();
    let decoded = pipeline
        .decode(b"414243>", 16, 0, &mut Diagnostics::new())
        .unwrap();
    assert_eq!(&*decoded.data, b"ABC");
    let crypt_alone = dictionary(&[("Filter", name("Crypt"))]);
    let pipeline = Pipeline::from_dictionary(&crypt_alone).unwrap();
    let decoded = pipeline
        .decode(b"as is", 16, 0, &mut Diagnostics::new())
        .unwrap();
    assert_eq!(&*decoded.data, b"as is");
    // The cap holds with no decoding filter.
    assert!(matches!(
        pipeline.decode(b"as is", 4, 0, &mut Diagnostics::new()),
        Err(PdfError::DecodedTooLarge { cap: 4 })
    ));
}
