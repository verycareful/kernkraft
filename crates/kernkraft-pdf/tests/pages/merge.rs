// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

use kernkraft_fs::NoProgress;
use kernkraft_pdf::syntax::Dictionary;
use kernkraft_pdf::{merge, DiagnosticKind, PdfError};

use super::*;

fn merged(documents: &[&Document]) -> Document {
    merge(documents, &OPTIONS, &NoProgress).unwrap()
}

/// The annotation dictionaries of page `number`.
fn annotations(document: &Document, number: u32) -> Vec<Dictionary> {
    let page = document.page(number).unwrap();
    let annots = document.get(&page.dictionary, b"Annots").unwrap();
    annots
        .array()
        .unwrap_or_default()
        .iter()
        .map(|annot| {
            document
                .resolve(annot)
                .unwrap()
                .dictionary()
                .unwrap()
                .clone()
        })
        .collect()
}

fn diagnostics(document: &Document) -> Vec<DiagnosticKind> {
    document
        .diagnostics()
        .into_iter()
        .map(|diagnostic| diagnostic.kind)
        .collect()
}

#[test]
fn pages_follow_one_another() {
    let a = open(&pdf_n(2));
    let b = open(&pdf(&[(100.0, 50.0)]));
    let c = open(&pdf_n(1));
    let all = merged(&[&a, &b, &c]);
    assert_eq!(all.page_count().unwrap(), 4);
    assert_eq!(markers(&all), ["P1", "P2", "P1", "P1"]);
    assert_eq!(all.page(3).unwrap().media_box.unwrap().height(), 50.0);
    let again = reopen(&all);
    assert_eq!(markers(&again), ["P1", "P2", "P1", "P1"]);
    assert_eq!(again.page(3).unwrap().media_box.unwrap().width(), 100.0);
}

#[test]
fn fewer_than_two_inputs_are_refused() {
    let a = open(&pdf_n(1));
    assert!(matches!(
        merge(&[&a], &OPTIONS, &NoProgress),
        Err(PdfError::Invalid(_))
    ));
    assert!(matches!(
        merge(&[], &OPTIONS, &NoProgress),
        Err(PdfError::Invalid(_))
    ));
}

#[test]
fn the_inputs_are_not_changed() {
    let a = open(&features("A").bytes);
    let b = open(&features("B").bytes);
    let before = (saved(&a), saved(&b));
    let _ = merged(&[&a, &b]);
    assert_eq!((saved(&a), saved(&b)), before);
}

#[test]
fn nested_pages_keep_their_inherited_attributes() {
    let nested = open(&nested_pdf());
    let flat = open(&pdf_n(1));
    let both = merged(&[&flat, &nested]);
    for (number, source) in [(2, 1), (3, 2), (4, 3)] {
        let before = nested.page(source).unwrap();
        let after = both.page(number).unwrap();
        assert_eq!(after.media_box, before.media_box);
        assert_eq!(after.crop_box, before.crop_box);
        assert_eq!(after.rotate, before.rotate);
    }
    assert_eq!(reopen(&both).page(3).unwrap().rotate, 90);
}

#[test]
fn the_merged_file_has_the_highest_version() {
    let a = open(&pdf_n(1));
    let b = open(&pdf_n(1));
    let bytes = saved(&merged(&[&a, &b]));
    assert!(bytes.starts_with(b"%PDF-1.7"));
}

#[test]
fn the_outline_of_each_input_follows_the_one_before() {
    let a = open(&features("A").bytes);
    let b = open(&features("B").bytes);
    let both = merged(&[&a, &b]);
    let expected = [
        (0, "AOne", 1),
        (1, "AOne.A", 2),
        (0, "ATwo", 2),
        (0, "AThree", 3),
        (0, "BOne", 4),
        (1, "BOne.A", 5),
        (0, "BTwo", 5),
        (0, "BThree", 6),
    ]
    .map(|(level, title, page)| (level, title.to_string(), Some(page)));
    assert_eq!(outline(&both), expected);
    assert_eq!(outline(&reopen(&both)), expected);
}

#[test]
fn named_destinations_of_all_inputs_are_in_one_tree() {
    let a = open(&features("A").bytes);
    let b = open(&features("B").bytes);
    let both = reopen(&merged(&[&a, &b]));
    assert_eq!(
        dest_names(&both),
        ["A-p1", "A-p2", "A-p3", "B-p1", "B-p2", "B-p3", "Bold2", "Aold2"]
            .iter()
            .map(|name| name.to_string())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
    );
    // The link of page 6 goes to B's page 1 by its name.
    let link = annotations(&both, 6).pop().unwrap();
    let action = link.get(b"A").unwrap();
    let Value::Dictionary(action) = action.value() else {
        panic!("an action");
    };
    assert_eq!(
        action.get(b"D").unwrap().value(),
        &Value::String(kernkraft_pdf::syntax::PdfString::new(&b"B-p1"[..]))
    );
}

/// Two inputs that name the same destination: the later one's name gets
/// a suffix and the later input's references follow it.
#[test]
fn a_name_that_two_inputs_have_is_changed_in_the_later_one() {
    let a = open(&features("").bytes);
    let b = open(&features("").bytes);
    let both = reopen(&merged(&[&a, &b]));
    assert_eq!(
        dest_names(&both),
        ["-p1", "-p1+1", "-p2", "-p2+1", "-p3", "-p3+1", "old2", "old2+1"]
    );
    // "Three" of the second input is a destination by name: it led to the
    // second input's page 3, which is page 6.
    let items = outline(&both);
    assert_eq!(items[3], (0, "Three".to_string(), Some(3)));
    assert_eq!(items[7], (0, "Three".to_string(), Some(6)));
    // The link on the second input's page 3 (page 6) names "-p1+1".
    let link = annotations(&both, 6).pop().unwrap();
    let Value::Dictionary(action) = link.get(b"A").unwrap().value() else {
        panic!("an action");
    };
    assert_eq!(
        action.get(b"D").unwrap().value(),
        &Value::String(kernkraft_pdf::syntax::PdfString::new(&b"-p1+1"[..]))
    );
}

#[test]
fn the_fields_follow_one_another_and_a_repeated_name_is_changed() {
    let a = open(&features("A").bytes);
    let b = open(&features("B").bytes);
    assert_eq!(
        field_names(&reopen(&merged(&[&a, &b]))),
        ["A_f2", "A_f3", "B_f2", "B_f3"]
    );
    let c = open(&features("A").bytes);
    assert_eq!(
        field_names(&reopen(&merged(&[&a, &c]))),
        ["A_f2", "A_f3", "A_f2+1", "A_f3+1"]
    );
}

#[test]
fn the_form_keeps_its_defaults_and_order() {
    let a = open(&features("A").bytes);
    let b = open(&features("B").bytes);
    let both = merged(&[&a, &b]);
    let form = resolved_entry(&both, b"AcroForm");
    let form = form.dictionary().unwrap();
    assert_eq!(
        both.get(form, b"DA").unwrap().value(),
        &Value::String(kernkraft_pdf::syntax::PdfString::new(
            &b"/Helv 0 Tf 0 g"[..]
        ))
    );
    assert_eq!(both.get(form, b"CO").unwrap().array().unwrap().len(), 4);
    let resources = both.get(form, b"DR").unwrap();
    let fonts = both.get(resources.dictionary().unwrap(), b"Font").unwrap();
    assert!(fonts.dictionary().unwrap().get(b"Helv").is_some());
    // Both inputs' /Helv is the same font, whose object each input has
    // under its own number: the one that stays is the first's, and a
    // reader is told.
    assert!(diagnostics(&both)
        .iter()
        .any(|kind| matches!(kind, DiagnosticKind::ResourceNameKept { .. })));
}

#[test]
fn page_labels_move_with_their_input() {
    let a = open(&features("A").bytes);
    let b = open(&features("B").bytes);
    let both = merged(&[&a, &b]);
    let expected = ["i", "AA-1", "AA-2", "i", "BA-1", "BA-2"];
    assert_eq!(labels(&both).unwrap(), expected);
    assert_eq!(labels(&reopen(&both)).unwrap(), expected);
}

/// An input with no labels is numbered from 1 as it was in its own file,
/// after one that has them.
#[test]
fn an_input_without_labels_is_numbered_from_one() {
    let a = open(&features("A").bytes);
    let plain = open(&pdf_n(2));
    assert_eq!(
        labels(&merged(&[&a, &plain])).unwrap(),
        ["i", "AA-1", "AA-2", "1", "2"]
    );
    assert_eq!(
        labels(&merged(&[&plain, &a])).unwrap(),
        ["1", "2", "i", "AA-1", "AA-2"]
    );
    assert!(labels(&merged(&[&plain, &open(&pdf_n(1))])).is_none());
}

#[test]
fn the_structure_trees_are_one_and_their_keys_do_not_meet() {
    let a = open(&features("A").bytes);
    let b = open(&features("B").bytes);
    let both = reopen(&merged(&[&a, &b]));
    assert_eq!(
        structure_pages(&both),
        [Some(1), Some(2), Some(3), Some(4), Some(5), Some(6)]
    );
    assert_eq!(parent_tree_keys(&both), [0, 1, 2, 3, 4, 5]);
    // The pages' own keys moved with the tree's.
    let keys: Vec<Value> = (1..=6)
        .map(|number| {
            both.page(number)
                .unwrap()
                .dictionary
                .get(b"StructParents")
                .unwrap()
                .value()
                .clone()
        })
        .collect();
    assert_eq!(keys, (0..6).map(Value::Integer).collect::<Vec<_>>());
    let mark = resolved_entry(&both, b"MarkInfo");
    assert_eq!(
        mark.dictionary().unwrap().get(b"Marked").unwrap().value(),
        &Value::Boolean(true)
    );
}

#[test]
fn the_structure_parent_of_an_element_is_the_merged_root() {
    let a = open(&features("A").bytes);
    let b = open(&features("B").bytes);
    let both = reopen(&merged(&[&a, &b]));
    let root = both
        .catalog()
        .unwrap()
        .dictionary()
        .unwrap()
        .get(b"StructTreeRoot")
        .unwrap()
        .clone();
    let Value::Reference(root_id) = root.value() else {
        panic!("a reference");
    };
    let resolved = both.resolve(&root).unwrap();
    let k = both.get(resolved.dictionary().unwrap(), b"K").unwrap();
    for child in k.array().unwrap() {
        let element = both.resolve(child).unwrap();
        assert_eq!(
            element.dictionary().unwrap().get(b"P").unwrap().value(),
            &Value::Reference(*root_id)
        );
    }
}

#[test]
fn attachments_of_all_inputs_are_kept() {
    let a = open(&features("A").bytes);
    let b = open(&features("B").bytes);
    let both = reopen(&merged(&[&a, &b]));
    assert_eq!(
        names_of(&both, b"EmbeddedFiles"),
        ["A-att.txt", "B-att.txt"]
    );
    let c = open(&features("A").bytes);
    let twice = reopen(&merged(&[&a, &c]));
    assert_eq!(
        names_of(&twice, b"EmbeddedFiles"),
        ["A-att.txt", "A-att.txt+1"]
    );
    let attachments = twice.attachments().unwrap();
    assert_eq!(attachments.len(), 2);
}

#[test]
fn the_first_input_gives_the_open_action() {
    let a = open(&features("A").bytes);
    let b = open(&features("B").bytes);
    let both = merged(&[&a, &b]);
    let catalog = both.catalog().unwrap();
    let action = catalog.dictionary().unwrap().get(b"OpenAction").unwrap();
    let Value::Array(items) = action.value() else {
        panic!("a destination");
    };
    assert_eq!(
        items[0].value(),
        &Value::Reference(both.page_id(3).unwrap())
    );
}

#[test]
fn what_is_not_carried_is_reported() {
    let info = flat_with(
        &[(100.0, 100.0)],
        "/Metadata 31 0 R /Lang (en)",
        "",
        "/Info 30 0 R",
        &[(30, "<< /Title (Doc) >>"), (31, "<< /Type /Metadata >>")],
    );
    let a = open(&info.bytes);
    let b = open(&info.bytes);
    let both = merged(&[&a, &b]);
    let kinds = diagnostics(&both);
    let not_carried = |key: &str| {
        kinds.iter().any(|kind| {
            matches!(kind, DiagnosticKind::CatalogEntryNotCarried { key: found } if found.as_bytes() == key.as_bytes())
        })
    };
    assert!(not_carried("Metadata"));
    assert!(not_carried("Info"));
    // `Lang` of the first input is carried; the second's is not.
    assert!(not_carried("Lang"));
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| matches!(kind, DiagnosticKind::CatalogEntryNotCarried { key } if key.as_bytes() == b"Lang"))
            .count(),
        1
    );
    let lang = both
        .catalog()
        .unwrap()
        .dictionary()
        .unwrap()
        .get(b"Lang")
        .cloned();
    assert!(lang.is_some());
}

#[test]
fn an_encrypted_input_is_read_and_the_result_is_not_encrypted() {
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testdata/encrypted/rc4-128.pdf"),
    )
    .unwrap();
    let locked = Document::from_bytes(bytes, &OPTIONS, "user").unwrap();
    let plain = open(&pdf_n(1));
    let both = merged(&[&plain, &locked]);
    assert_eq!(both.page_count().unwrap(), 2);
    assert!(diagnostics(&both).contains(&DiagnosticKind::EncryptionNotCarried));
    let written = saved(&both);
    let reopened = Document::from_bytes(written, &OPTIONS, "").unwrap();
    assert_eq!(reopened.page_count().unwrap(), 2);
}

#[test]
fn optional_content_keeps_each_groups_state() {
    let first = flat_with(
        &[(100.0, 100.0)],
        "/OCProperties << /OCGs [30 0 R 31 0 R] /D << /OFF [31 0 R] /Order [30 0 R 31 0 R] /Name (First) >> >>",
        "",
        "",
        &[
            (30, "<< /Type /OCG /Name (one) >>"),
            (31, "<< /Type /OCG /Name (two) >>"),
        ],
    );
    let second = flat_with(
        &[(100.0, 100.0)],
        "/OCProperties << /OCGs [30 0 R 31 0 R] /D << /BaseState /OFF /ON [30 0 R] >> >>",
        "",
        "",
        &[
            (30, "<< /Type /OCG /Name (three) >>"),
            (31, "<< /Type /OCG /Name (four) >>"),
        ],
    );
    let both = reopen(&merged(&[&open(&first.bytes), &open(&second.bytes)]));
    let properties = resolved_entry(&both, b"OCProperties");
    let properties = properties.dictionary().unwrap();
    let groups = both.get(properties, b"OCGs").unwrap();
    let name_of = |group: &Object| {
        let group = both.resolve(group).unwrap();
        match both
            .get(group.dictionary().unwrap(), b"Name")
            .unwrap()
            .value()
        {
            Value::String(name) => String::from_utf8_lossy(name.as_bytes()).into_owned(),
            other => panic!("{other:?}"),
        }
    };
    let names: Vec<String> = groups.array().unwrap().iter().map(name_of).collect();
    assert_eq!(names, ["one", "two", "three", "four"]);
    let config = both.get(properties, b"D").unwrap();
    let off: Vec<String> = both
        .get(config.dictionary().unwrap(), b"OFF")
        .unwrap()
        .array()
        .unwrap()
        .iter()
        .map(name_of)
        .collect();
    // "two" was off; the second input's base state was off with "three" on.
    assert_eq!(off, ["two", "four"]);
}

#[test]
fn a_reference_to_nothing_stays_nothing() {
    // Page 1 refers to an object past the last the file has.
    let fixture = flat_with(&[(100.0, 100.0)], "", "/Thumb 900 0 R", "", &[]);
    let a = Document::from_bytes(fixture.bytes, &OPTIONS, "").unwrap();
    let b = open(&pdf_n(2));
    let both = reopen(&merged(&[&a, &b]));
    assert_eq!(markers(&both), ["P1", "P1", "P2"]);
    let page = both.page(1).unwrap();
    let thumb = both.get(&page.dictionary, b"Thumb").unwrap();
    assert!(matches!(thumb.value(), Value::Null));
}

#[test]
fn merging_can_be_cancelled() {
    struct Cancelled;
    impl kernkraft_fs::Progress for Cancelled {
        fn report(&self, _: u64, _: u64, _: &str) {}
        fn is_cancelled(&self) -> bool {
            true
        }
    }
    let a = open(&pdf_n(1));
    let b = open(&pdf_n(1));
    assert!(matches!(
        merge(&[&a, &b], &OPTIONS, &Cancelled),
        Err(PdfError::Cancelled)
    ));
}
