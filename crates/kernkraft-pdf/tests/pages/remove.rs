// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Remove and extract: the pages that go, and what said something about
//! them.

use kernkraft_pdf::{extract, remove, PdfError};

use super::*;

fn contains(bytes: &[u8], needle: &str) -> bool {
    bytes
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

#[test]
fn removes_the_pages_and_what_only_they_used() {
    let document = open(&pdf_n(3));
    let fewer = remove(&document, &[2]).unwrap();
    assert_eq!(markers(&fewer), ["P1", "P3"]);
    assert_eq!(fewer.page_count().unwrap(), 2);
    let bytes = saved(&fewer);
    assert!(!contains(&bytes, "(P2)"), "page 2's content is not written");
    assert_eq!(markers(&open(&bytes)), ["P1", "P3"]);
    assert_eq!(markers(&document), ["P1", "P2", "P3"], "the original stays");
}

#[test]
fn removing_nothing_or_everything_or_a_missing_page_is_refused() {
    let document = open(&pdf_n(2));
    assert!(matches!(remove(&document, &[]), Err(PdfError::Invalid(_))));
    assert!(matches!(
        remove(&document, &[1, 2]),
        Err(PdfError::Invalid(_))
    ));
    assert!(matches!(
        remove(&document, &[3]),
        Err(PdfError::PageOutOfRange { page: 3, count: 2 })
    ));
    assert!(matches!(
        remove(&document, &[1, 1]),
        Err(PdfError::DuplicatePage(1))
    ));
}

#[test]
fn extract_keeps_the_order_given() {
    let document = open(&pdf_n(4));
    let part = extract(&document, &[4, 2]).unwrap();
    assert_eq!(markers(&part), ["P4", "P2"]);
    assert_eq!(markers(&reopen(&part)), ["P4", "P2"]);
}

#[test]
fn extract_refuses_nothing_a_missing_page_and_a_page_twice() {
    let document = open(&pdf_n(2));
    assert!(matches!(extract(&document, &[]), Err(PdfError::Invalid(_))));
    assert!(matches!(
        extract(&document, &[0]),
        Err(PdfError::PageOutOfRange { page: 0, .. })
    ));
    assert!(matches!(
        extract(&document, &[2, 2]),
        Err(PdfError::DuplicatePage(2))
    ));
}

#[test]
fn nested_pages_keep_their_inherited_attributes() {
    let document = open(&nested_pdf());
    let part = extract(&document, &[3]).unwrap();
    let before = document.page(3).unwrap();
    let after = part.page(1).unwrap();
    assert_eq!(after.media_box, before.media_box);
    assert_eq!(after.crop_box, before.crop_box);
    assert_eq!(after.rotate, 90);
    assert!(after.resources.is_some());
    let again = reopen(&part).page(1).unwrap();
    assert_eq!(again.media_box, before.media_box);
    assert_eq!(again.rotate, 90);
}

#[test]
fn the_outline_loses_the_items_of_a_removed_page() {
    let document = open(&features("").bytes);
    assert_eq!(
        outline(&document),
        [
            (0, "One".to_string(), Some(1)),
            (1, "One.A".to_string(), Some(2)),
            (0, "Two".to_string(), Some(2)),
            (0, "Three".to_string(), Some(3)),
        ]
    );
    let fewer = remove(&document, &[2]).unwrap();
    let expected = [
        (0, "One".to_string(), Some(1)),
        (0, "Three".to_string(), Some(2)),
    ];
    assert_eq!(outline(&fewer), expected);
    assert_eq!(outline(&reopen(&fewer)), expected);
}

/// The items below an item that goes are not lost with it.
#[test]
fn the_children_of_a_removed_item_move_up() {
    let document = open(&features("").bytes);
    let fewer = remove(&document, &[1]).unwrap();
    assert_eq!(
        outline(&reopen(&fewer)),
        [
            (0, "One.A".to_string(), Some(1)),
            (0, "Two".to_string(), Some(1)),
            (0, "Three".to_string(), Some(2)),
        ]
    );
}

#[test]
fn the_outline_counts_and_links_are_written_again() {
    let document = open(&features("").bytes);
    let fewer = remove(&document, &[2]).unwrap();
    let root = resolved_entry(&fewer, b"Outlines");
    let root = root.dictionary().unwrap();
    assert_eq!(root.get(b"Count").unwrap().value(), &Value::Integer(2));
    let first = root.get(b"First").unwrap();
    let Value::Reference(first_id) = first.value() else {
        panic!("First is a reference");
    };
    assert_eq!(*first_id, id(61));
    let one = fewer.required(id(61)).unwrap();
    let one = one.dictionary().unwrap();
    assert!(one.get(b"First").is_none(), "its only child went");
    assert!(one.get(b"Count").is_none());
    assert_eq!(one.get(b"Next").unwrap().value(), &Value::Reference(id(63)));
    let three = fewer.required(id(63)).unwrap();
    assert_eq!(
        three.dictionary().unwrap().get(b"Prev").unwrap().value(),
        &Value::Reference(id(61))
    );
}

#[test]
fn a_part_keeps_the_items_that_lead_into_it() {
    let document = open(&features("").bytes);
    // Pages 1 and 2 hold every item but "Three"; keep page 2 only: "Two"
    // and "One.A" stay and "One" and "Three" go.
    let part = extract(&document, &[2]).unwrap();
    assert_eq!(
        outline(&part),
        [
            (0, "One.A".to_string(), Some(1)),
            (0, "Two".to_string(), Some(1)),
        ]
    );
}

/// A heading that leads nowhere stays while something below it does.
#[test]
fn a_heading_stays_while_an_item_below_it_does() {
    let extra = [
        (
            30,
            "<< /Type /Outlines /First 31 0 R /Last 31 0 R /Count 3 >>",
        ),
        (
            31,
            "<< /Title (Heading) /Parent 30 0 R /First 32 0 R /Last 33 0 R /Count 2 >>",
        ),
        (
            32,
            "<< /Title (Gone) /Parent 31 0 R /Next 33 0 R /Dest [10 0 R /Fit] >>",
        ),
        (
            33,
            "<< /Title (Kept) /Parent 31 0 R /Prev 32 0 R /Dest [12 0 R /Fit] >>",
        ),
    ];
    let fixture = flat_with(
        &[(100.0, 100.0), (100.0, 100.0)],
        "/Outlines 30 0 R",
        "",
        "",
        &extra,
    );
    let document = open(&fixture.bytes);
    assert_eq!(
        outline(&document),
        [
            (0, "Heading".to_string(), None),
            (1, "Gone".to_string(), Some(1)),
            (1, "Kept".to_string(), Some(2)),
        ]
    );
    let after = remove(&document, &[1]).unwrap();
    assert_eq!(
        outline(&after),
        [
            (0, "Heading".to_string(), None),
            (1, "Kept".to_string(), Some(1))
        ]
    );
    let none = remove(&document, &[2]).unwrap();
    assert_eq!(
        outline(&none),
        [
            (0, "Heading".to_string(), None),
            (1, "Gone".to_string(), Some(1))
        ]
    );
}

#[test]
fn an_outline_none_of_whose_items_stay_leaves_the_catalog() {
    let extra = [
        (
            30,
            "<< /Type /Outlines /First 31 0 R /Last 31 0 R /Count 1 >>",
        ),
        (31, "<< /Title (Only) /Parent 30 0 R /Dest [10 0 R /Fit] >>"),
    ];
    let fixture = flat_with(
        &[(100.0, 100.0), (100.0, 100.0)],
        "/Outlines 30 0 R",
        "",
        "",
        &extra,
    );
    let after = remove(&open(&fixture.bytes), &[1]).unwrap();
    assert!(after.outline().unwrap().is_empty());
    assert!(after
        .catalog()
        .unwrap()
        .dictionary()
        .unwrap()
        .get(b"Outlines")
        .is_none());
}

#[test]
fn named_destinations_to_removed_pages_are_taken_out() {
    let document = open(&features("").bytes);
    assert_eq!(dest_names(&document), ["-p1", "-p2", "-p3"]);
    let fewer = remove(&document, &[2]).unwrap();
    assert_eq!(dest_names(&fewer), ["-p1", "-p3"]);
    assert_eq!(dest_names(&reopen(&fewer)), ["-p1", "-p3"]);
    // The catalog's own `Dests` dictionary lost `old2`.
    let legacy = resolved_entry(&fewer, b"Dests");
    assert!(legacy.dictionary().unwrap().get(b"old2").is_none());
}

#[test]
fn a_name_tree_with_nothing_left_leaves_the_name_dictionary() {
    let document = open(&features("").bytes);
    let part = extract(&document, &[1]).unwrap();
    assert_eq!(dest_names(&part), ["-p1"]);
    let tiny = flat_with(
        &[(10.0, 10.0), (10.0, 10.0)],
        "/Names << /Dests 30 0 R >>",
        "",
        "",
        &[(30, "<< /Names [(only) [12 0 R /Fit]] >>")],
    );
    let after = remove(&open(&tiny.bytes), &[2]).unwrap();
    let names = resolved_entry(&after, b"Names");
    assert!(names.dictionary().unwrap().get(b"Dests").is_none());
}

#[test]
fn the_form_loses_the_fields_of_a_removed_page() {
    let document = open(&features("").bytes);
    assert_eq!(field_names(&document), ["_f2", "_f3"]);
    let fewer = remove(&document, &[2]).unwrap();
    assert_eq!(field_names(&fewer), ["_f3"]);
    assert_eq!(field_names(&reopen(&fewer)), ["_f3"]);
    let form = resolved_entry(&fewer, b"AcroForm");
    let order = fewer.get(form.dictionary().unwrap(), b"CO").unwrap();
    assert_eq!(order.array().unwrap().len(), 1);
    let bytes = saved(&fewer);
    assert!(!contains(&bytes, "_f2"), "the field is not written");
}

#[test]
fn a_field_stays_while_one_widget_does() {
    let extra = [
        (30, "<< /FT /Tx /T (parent) /Kids [31 0 R 32 0 R] >>"),
        (
            31,
            "<< /Type /Annot /Subtype /Widget /Parent 30 0 R /Rect [0 0 1 1] >>",
        ),
        (
            32,
            "<< /Type /Annot /Subtype /Widget /Parent 30 0 R /Rect [0 0 1 1] >>",
        ),
    ];
    let fixture = flat_with(
        &[(100.0, 100.0), (100.0, 100.0)],
        "/AcroForm << /Fields [30 0 R] >>",
        "",
        "",
        &extra,
    );
    let mut text = fixture.bytes;
    // Page 1 shows widget 31 and page 2 widget 32.
    for (page, widget) in [("/Contents 11 0 R", 31), ("/Contents 13 0 R", 32)] {
        let at = text
            .windows(page.len())
            .position(|w| w == page.as_bytes())
            .unwrap();
        text.splice(at..at, format!("/Annots [{widget} 0 R] ").bytes());
    }
    let document = Document::from_bytes(text, &OPTIONS, "")
        .and_then(|d| d.rebuilt())
        .unwrap();
    let after = remove(&document, &[2]).unwrap();
    assert_eq!(field_names(&after), ["parent"]);
    let parent = after.required(id(30)).unwrap();
    let kids = parent.dictionary().unwrap().get(b"Kids").unwrap();
    assert_eq!(kids.value(), &Value::Array(vec![reference(31)]));
    let both = remove(&document, &[1]).unwrap();
    assert_eq!(field_names(&both), ["parent"]);
}

fn reference(number: u32) -> Object {
    Object::new(Value::Reference(id(number)))
}

#[test]
fn the_structure_tree_loses_the_elements_of_a_removed_page() {
    let document = open(&features("").bytes);
    assert_eq!(structure_pages(&document), [Some(1), Some(2), Some(3)]);
    let fewer = remove(&document, &[2]).unwrap();
    assert_eq!(structure_pages(&fewer), [Some(1), Some(2)]);
    assert_eq!(parent_tree_keys(&fewer), [0, 2]);
    let again = reopen(&fewer);
    assert_eq!(structure_pages(&again), [Some(1), Some(2)]);
    assert_eq!(parent_tree_keys(&again), [0, 2]);
}

#[test]
fn an_element_keeps_the_content_that_stays() {
    let extra = [
        (
            30,
            "<< /Type /StructTreeRoot /K 31 0 R /ParentTree 35 0 R >>",
        ),
        (
            31,
            "<< /Type /StructElem /S /Document /P 30 0 R /K [32 0 R 33 0 R] >>",
        ),
        (
            32,
            "<< /Type /StructElem /S /P /P 31 0 R /Pg 10 0 R /K [0 << /Type /MCR /Pg 12 0 R /MCID 1 >>] >>",
        ),
        (
            33,
            "<< /Type /StructElem /S /P /P 31 0 R /Pg 12 0 R /K 2 >>",
        ),
        (35, "<< /Nums [0 [32 0 R] 1 [32 0 R 33 0 R]] >>"),
    ];
    let fixture = flat_with(
        &[(100.0, 100.0), (100.0, 100.0)],
        "/StructTreeRoot 30 0 R",
        "",
        "",
        &extra,
    );
    let mut text = fixture.bytes;
    for (page, key) in [("/Contents 11 0 R", 0), ("/Contents 13 0 R", 1)] {
        let at = text
            .windows(page.len())
            .position(|w| w == page.as_bytes())
            .unwrap();
        text.splice(at..at, format!("/StructParents {key} ").bytes());
    }
    let document = Document::from_bytes(text, &OPTIONS, "")
        .and_then(|d| d.rebuilt())
        .unwrap();
    let after = remove(&document, &[2]).unwrap();
    // Element 32 keeps its integer child; its content reference to page 2
    // goes; element 33 had content only on page 2 and goes.
    let kept = after.required(id(32)).unwrap();
    assert_eq!(
        kept.dictionary().unwrap().get(b"K").unwrap().value(),
        &Value::Array(vec![Object::new(Value::Integer(0))])
    );
    let group = after.required(id(31)).unwrap();
    assert_eq!(
        group.dictionary().unwrap().get(b"K").unwrap().value(),
        &Value::Array(vec![reference(32)])
    );
    assert_eq!(parent_tree_keys(&after), [0]);
    let parents = NumberTree::new(
        &after,
        after
            .required(id(30))
            .unwrap()
            .dictionary()
            .unwrap()
            .get(b"ParentTree")
            .unwrap()
            .clone(),
    )
    .get(0)
    .unwrap()
    .unwrap();
    assert_eq!(after.resolve(&parents).unwrap().array().unwrap().len(), 1);
}

/// An element whose whole content is on removed pages goes with its
/// parent when nothing else is left in it.
#[test]
fn an_element_with_nothing_left_leaves_its_parent() {
    let extra = [
        (30, "<< /Type /StructTreeRoot /K 31 0 R >>"),
        (31, "<< /Type /StructElem /S /Sect /P 30 0 R /K 32 0 R >>"),
        (
            32,
            "<< /Type /StructElem /S /P /P 31 0 R /Pg 12 0 R /K 0 >>",
        ),
    ];
    let fixture = flat_with(
        &[(100.0, 100.0), (100.0, 100.0)],
        "/StructTreeRoot 30 0 R",
        "",
        "",
        &extra,
    );
    let after = remove(&open(&fixture.bytes), &[2]).unwrap();
    let root = after.required(id(30)).unwrap();
    assert!(root.dictionary().unwrap().get(b"K").is_none());
}

#[test]
fn page_labels_follow_the_pages() {
    let document = open(&features("").bytes);
    let fewer = remove(&document, &[2]).unwrap();
    assert_eq!(labels(&fewer).unwrap(), ["i", "A-2"]);
    assert_eq!(labels(&reopen(&fewer)).unwrap(), ["i", "A-2"]);
    let first = remove(&document, &[1]).unwrap();
    assert_eq!(labels(&first).unwrap(), ["A-1", "A-2"]);
    let part = extract(&document, &[3, 2]).unwrap();
    assert_eq!(labels(&part).unwrap(), ["A-2", "A-1"]);
}

#[test]
fn links_to_removed_pages_lose_their_destination() {
    let document = open(&features("").bytes);
    // Page 1's link goes to page 3; the open action goes to page 3.
    let fewer = remove(&document, &[3]).unwrap();
    let link = fewer.required(id(21)).unwrap();
    assert!(link.dictionary().unwrap().get(b"Dest").is_none());
    assert!(fewer
        .catalog()
        .unwrap()
        .dictionary()
        .unwrap()
        .get(b"OpenAction")
        .is_none());
    // The go-to action of a link on page 3 is taken out with the page; a
    // link that stays and leads to a page that stays is not touched.
    let kept = remove(&document, &[2]).unwrap();
    let link = kept.required(id(21)).unwrap();
    assert!(link.dictionary().unwrap().get(b"Dest").is_some());
}

#[test]
fn a_go_to_action_to_a_removed_page_is_taken_out() {
    let extra = [(
        30,
        "<< /Type /Annot /Subtype /Link /Rect [0 0 1 1] /A << /S /GoTo /D [12 0 R /Fit] /Next << /S /URI /URI (http://example.invalid/) >> >> >>",
    )];
    let fixture = flat_with(&[(100.0, 100.0), (100.0, 100.0)], "", "", "", &extra);
    let mut text = fixture.bytes;
    let key = "/Contents 11 0 R";
    let at = text
        .windows(key.len())
        .position(|w| w == key.as_bytes())
        .unwrap();
    text.splice(at..at, "/Annots [30 0 R] ".bytes());
    let document = Document::from_bytes(text, &OPTIONS, "")
        .and_then(|d| d.rebuilt())
        .unwrap();
    let after = remove(&document, &[2]).unwrap();
    let link = after.required(id(30)).unwrap();
    // The URI action the go-to chained to takes its place.
    let action = link.dictionary().unwrap().get(b"A").unwrap();
    let action = action.value();
    let Value::Dictionary(action) = action else {
        panic!("an action: {action:?}");
    };
    assert_eq!(
        action.get(b"S").unwrap().value(),
        &Value::Name(kernkraft_pdf::syntax::Name::new(&b"URI"[..]))
    );
}

#[test]
fn another_reference_to_a_removed_page_becomes_null() {
    let extra = [(
        30,
        "<< /Type /Annot /Subtype /Text /Rect [0 0 1 1] /P 12 0 R >>",
    )];
    let fixture = flat_with(&[(100.0, 100.0), (100.0, 100.0)], "", "", "", &extra);
    let mut text = fixture.bytes;
    let key = "/Contents 11 0 R";
    let at = text
        .windows(key.len())
        .position(|w| w == key.as_bytes())
        .unwrap();
    text.splice(at..at, "/Annots [30 0 R] ".bytes());
    let document = Document::from_bytes(text, &OPTIONS, "")
        .and_then(|d| d.rebuilt())
        .unwrap();
    let after = remove(&document, &[2]).unwrap();
    let note = after.required(id(30)).unwrap();
    assert_eq!(
        note.dictionary().unwrap().get(b"P").unwrap().value(),
        &Value::Null
    );
}

#[test]
fn an_encrypted_document_stays_encrypted() {
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testdata/encrypted/aes-128.pdf"),
    )
    .unwrap();
    let document = Document::from_bytes(bytes, &OPTIONS, "user").unwrap();
    let part = extract(&document, &[1]).unwrap();
    let written = saved(&part);
    assert!(matches!(
        Document::from_bytes(written.clone(), &OPTIONS, ""),
        Err(PdfError::WrongPassword)
    ));
    let reopened = Document::from_bytes(written, &OPTIONS, "user").unwrap();
    assert_eq!(reopened.page_count().unwrap(), 1);
}

#[test]
fn the_original_is_not_changed_by_what_is_made_from_it() {
    let document = open(&features("").bytes);
    let before = saved(&document);
    let _ = remove(&document, &[2]).unwrap();
    let _ = extract(&document, &[3]).unwrap();
    assert_eq!(saved(&document), before);
}
