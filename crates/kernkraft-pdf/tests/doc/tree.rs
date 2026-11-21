// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Name and number trees (`doc/tree.rs`).

use super::*;
use kernkraft_pdf::doc::{Document, NameTree, NumberTree};
use kernkraft_pdf::syntax::Value;
use kernkraft_pdf::PdfError;

/// The tree of 7.9.6, Example 1, cut down: a root, two intermediate
/// nodes, and leaves whose keys are element names.
fn elements() -> Document {
    table_file(
        &[
            (1, &b"<< /Type /Catalog >>"[..]),
            (2, b"<< /Kids [3 0 R 4 0 R] >>"),
            (3, b"<< /Limits [(Actinium) (Gold)] /Kids [5 0 R 6 0 R] >>"),
            (4, b"<< /Limits [(Hafnium) (Zirconium)] /Kids [7 0 R] >>"),
            (5, b"<< /Limits [(Actinium) (Astatine)] /Names [(Actinium) 89 (Aluminum) 13 (Astatine) 85] >>"),
            (6, b"<< /Limits [(Gold) (Gold)] /Names [(Gold) 79] >>"),
            (7, b"<< /Limits [(Hafnium) (Zirconium)] /Names [(Hafnium) 72 (Zirconium) 40] >>"),
        ],
        "/Root 1 0 R",
    )
    .open()
}

#[test]
fn name_tree_lookup_and_order() {
    let document = elements();
    let tree = NameTree::new(&document, reference(2));
    let number = |key: &[u8]| {
        tree.get(key)
            .unwrap()
            .map(|object| match object.into_value() {
                Value::Integer(integer) => integer,
                other => panic!("{other:?}"),
            })
    };
    assert_eq!(number(b"Gold"), Some(79));
    assert_eq!(number(b"Aluminum"), Some(13));
    assert_eq!(number(b"Zirconium"), Some(40));
    // Byte by byte: a shorter key is another key.
    assert_eq!(number(b"Gol"), None);
    assert_eq!(number(b"Zinc"), None);
    let keys: Vec<Vec<u8>> = tree
        .entries()
        .unwrap()
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    let expected: Vec<&[u8]> = vec![
        b"Actinium",
        b"Aluminum",
        b"Astatine",
        b"Gold",
        b"Hafnium",
        b"Zirconium",
    ];
    assert_eq!(keys, expected);
}

/// A child whose `Limits` are wrong is still searched when they do not
/// leave the key out, and one without `Limits` always is.
#[test]
fn missing_limits_do_not_hide_keys() {
    let document = table_file(
        &[
            (1, &b"<< /Type /Catalog >>"[..]),
            (2, b"<< /Kids [3 0 R] >>"),
            (3, b"<< /Names [(b) 2] >>"),
        ],
        "/Root 1 0 R",
    )
    .open();
    let tree = NameTree::new(&document, reference(2));
    assert!(tree.get(b"b").unwrap().is_some());
}

/// A direct root, as a catalog's `PageLabels` may hold one (12.4.2).
#[test]
fn number_tree() {
    let document = table_file(
        &[
            (1, &b"<< /Type /Catalog >>"[..]),
            (2, b"<< /Limits [0 9] /Nums [0 (a) 4 (b) 9 (c)] >>"),
            (3, b"<< /Limits [10 20] /Nums [10 (d)] >>"),
        ],
        "/Root 1 0 R",
    )
    .open();
    let root = Object::new(Value::Dictionary({
        let mut root = kernkraft_pdf::syntax::Dictionary::new();
        root.insert(
            kernkraft_pdf::syntax::Name::new(b"Kids".as_slice()),
            Object::new(Value::Array(vec![reference(2), reference(3)])),
        );
        root
    }));
    let tree = NumberTree::new(&document, root);
    assert_eq!(tree.get(4).unwrap(), Some(string(b"b")));
    assert_eq!(tree.get(10).unwrap(), Some(string(b"d")));
    assert_eq!(tree.get(5).unwrap(), None);
    let keys: Vec<i64> = tree
        .entries()
        .unwrap()
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    assert_eq!(keys, [0, 4, 9, 10]);
}

#[test]
fn malformed_trees_are_refused() {
    for (node, body) in [
        // A node that is its own kid.
        (&b"<< /Kids [2 0 R] >>"[..], ""),
        // An odd number of items.
        (b"<< /Names [(a) 1 (b)] >>", ""),
        // A key that is not a string.
        (b"<< /Names [/a 1] >>", ""),
        // A kid that is not a dictionary.
        (b"<< /Kids [3 0 R] >>", "42"),
    ] {
        let mut objects: Vec<(u32, &[u8])> = vec![(1, b"<< /Type /Catalog >>"), (2, node)];
        if !body.is_empty() {
            objects.push((3, body.as_bytes()));
        }
        let document = table_file(&objects, "/Root 1 0 R").open();
        let tree = NameTree::new(&document, reference(2));
        assert!(
            matches!(tree.entries(), Err(PdfError::InvalidTree { .. })),
            "{}",
            String::from_utf8_lossy(node)
        );
    }
}
