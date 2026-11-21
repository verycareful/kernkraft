// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

use kernkraft_pdf::syntax::{Dictionary, Name, Object, Real, Value};

use crate::{array, dict, int, null, string};

/// Enough entries that a lookup scanning the entries would cost
/// `MANY_KEYS * MANY_KEYS / 2` comparisons over the test.
const MANY_KEYS: usize = 200_000;

fn keys(dictionary: &Dictionary) -> Vec<Vec<u8>> {
    dictionary
        .iter()
        .map(|(key, _)| key.as_bytes().to_vec())
        .collect()
}

#[test]
fn real_refuses_non_finite() {
    assert!(Real::new(f64::NAN).is_none());
    assert!(Real::new(f64::INFINITY).is_none());
    assert!(Real::new(f64::NEG_INFINITY).is_none());
    assert_eq!(Real::new(f64::MAX).unwrap().get(), f64::MAX);
    assert_eq!(Real::new(-0.0).unwrap().get(), 0.0);
}

#[test]
fn new_object_has_no_span() {
    assert_eq!(int(1).span(), None);
    assert_eq!(Object::from(Value::Null).span(), None);
}

#[test]
fn a_built_object_is_edited_through_its_parent() {
    let mut object = array(vec![int(1)]);
    let Value::Array(items) = object.value_mut() else {
        panic!("built as an array");
    };
    *items[0].value_mut() = Value::Integer(2);
    assert_eq!(object, array(vec![int(2)]));
    assert_eq!(object.span(), None);
}

#[test]
fn objects_compare_by_value() {
    assert_eq!(
        array(vec![int(1), string(b"a")]),
        array(vec![int(1), string(b"a")])
    );
    assert_ne!(array(vec![int(1)]), array(vec![int(2)]));
    assert_ne!(int(1), null());
    assert_eq!(int(1).into_value(), Value::Integer(1));
}

#[test]
fn dictionary_keeps_insertion_order() {
    let object = dict(vec![("B", int(1)), ("A", int(2)), ("C", int(3))]);
    let Value::Dictionary(dictionary) = object.value() else {
        panic!("built as a dictionary");
    };
    assert_eq!(
        keys(dictionary),
        [b"B".to_vec(), b"A".to_vec(), b"C".to_vec()]
    );
    assert_eq!(dictionary.len(), 3);
    assert!(!dictionary.is_empty());
    assert!(Dictionary::new().is_empty());
}

#[test]
fn insert_of_an_existing_key_keeps_its_place() {
    let mut dictionary = Dictionary::new();
    assert_eq!(dictionary.insert(Name::new("A"), int(1)), None);
    assert_eq!(dictionary.insert(Name::new("B"), int(2)), None);
    assert_eq!(dictionary.insert(Name::new("A"), int(3)), Some(int(1)));
    assert_eq!(keys(&dictionary), [b"A".to_vec(), b"B".to_vec()]);
    assert_eq!(dictionary.get(b"A"), Some(&int(3)));
    assert_eq!(dictionary.len(), 2);
}

#[test]
fn remove_then_get_and_order() {
    let mut dictionary = Dictionary::new();
    dictionary.insert(Name::new("A"), int(1));
    dictionary.insert(Name::new("B"), int(2));
    dictionary.insert(Name::new("C"), int(3));
    assert_eq!(dictionary.remove(b"B"), Some(int(2)));
    assert_eq!(dictionary.remove(b"B"), None);
    assert_eq!(dictionary.get(b"B"), None);
    assert_eq!(keys(&dictionary), [b"A".to_vec(), b"C".to_vec()]);
    assert_eq!(dictionary.get(b"A"), Some(&int(1)));
    assert_eq!(dictionary.get(b"C"), Some(&int(3)));
    // The position of C moved; a later insert must still find it.
    assert_eq!(dictionary.insert(Name::new("C"), int(4)), Some(int(3)));
    assert_eq!(keys(&dictionary), [b"A".to_vec(), b"C".to_vec()]);
}

#[test]
fn get_mut_changes_the_entry() {
    let mut dictionary = Dictionary::new();
    dictionary.insert(Name::new("A"), int(1));
    *dictionary.get_mut(b"A").unwrap().value_mut() = Value::Integer(5);
    assert_eq!(dictionary.get(b"A"), Some(&int(5)));
    assert!(dictionary.get_mut(b"B").is_none());
}

#[test]
fn a_null_value_is_returned_as_written() {
    let mut dictionary = Dictionary::new();
    dictionary.insert(Name::new("A"), null());
    assert_eq!(dictionary.get(b"A"), Some(&null()));
    assert_eq!(dictionary.len(), 1);
}

#[test]
fn dictionaries_compare_in_order() {
    assert_eq!(
        dict(vec![("A", int(1)), ("B", int(2))]),
        dict(vec![("A", int(1)), ("B", int(2))])
    );
    assert_ne!(
        dict(vec![("A", int(1)), ("B", int(2))]),
        dict(vec![("B", int(2)), ("A", int(1))])
    );
}

#[test]
fn two_hundred_thousand_keys_insert_and_resolve() {
    let mut dictionary = Dictionary::new();
    for index in 0..MANY_KEYS {
        dictionary.insert(Name::new(format!("K{index}")), int(index as i64));
    }
    assert_eq!(dictionary.len(), MANY_KEYS);
    for index in 0..MANY_KEYS {
        assert_eq!(
            dictionary.get(format!("K{index}").as_bytes()),
            Some(&int(index as i64))
        );
    }
}
