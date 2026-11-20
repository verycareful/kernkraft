// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! `delete_annotation`: out of `Annots`, with its replies and pop-up, and
//! out of the structure tree. ISO 32000-1, 12.5.2 (Table 164: `Annots`,
//! `IRT`, `Popup`) and 14.7.4.3 (`OBJR`).

use super::*;
use kernkraft_pdf::{delete_annotation, PdfError};

const LINK: &[u8] = b"<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] >>";

fn note(extra: &str) -> Vec<u8> {
    format!("<< /Type /Annot /Subtype /Text /Rect [0 0 20 20] {extra} >>").into_bytes()
}

#[test]
fn the_annotation_leaves_annots_and_the_others_keep_their_order() {
    let fixture = annotated("", &[50, 51, 52], &[(50, LINK), (51, LINK), (52, LINK)]);
    let document = delete_annotation(&fixture.open(), 1, id(51)).unwrap();
    assert_eq!(annots_of(&document), vec![50, 52]);
    assert_eq!(document.annotations(1).unwrap().len(), 2);
}

#[test]
fn the_last_annotation_leaves_an_empty_array() {
    let fixture = annotated("", &[50], &[(50, LINK)]);
    let document = delete_annotation(&fixture.open(), 1, id(50)).unwrap();
    assert!(annots_of(&document).is_empty());
    assert!(document.annotations(1).unwrap().is_empty());
}

#[test]
fn an_annots_array_in_an_object_of_its_own_is_the_one_that_changes() {
    let fixture = document(
        "",
        "/Annots 50 0 R",
        "",
        &[(50, b"[51 0 R 52 0 R]".as_slice()), (51, LINK), (52, LINK)],
    );
    let document = delete_annotation(&fixture.open(), 1, id(51)).unwrap();
    assert_eq!(annots_of(&document), vec![52]);
    let page = dict(&document, page_object(0));
    assert!(page.get(b"Annots").is_some());
}

#[test]
fn replies_and_the_popup_go_with_the_annotation() {
    let parent = note("/Popup 52 0 R");
    let reply = note("/IRT 50 0 R");
    let reply_to_reply = note("/IRT 51 0 R");
    let popup = b"<< /Type /Annot /Subtype /Popup /Rect [30 0 130 100] /Parent 50 0 R >>";
    let other = note("");
    let fixture = annotated(
        "",
        &[50, 51, 52, 53, 54],
        &[
            (50, parent.as_slice()),
            (51, reply.as_slice()),
            (52, popup.as_slice()),
            (53, reply_to_reply.as_slice()),
            (54, other.as_slice()),
        ],
    );
    let document = delete_annotation(&fixture.open(), 1, id(50)).unwrap();
    assert_eq!(annots_of(&document), vec![54]);
}

#[test]
fn a_reply_alone_can_be_deleted_and_its_parent_stays() {
    let parent = note("");
    let reply = note("/IRT 50 0 R");
    let fixture = annotated(
        "",
        &[50, 51],
        &[(50, parent.as_slice()), (51, reply.as_slice())],
    );
    let document = delete_annotation(&fixture.open(), 1, id(51)).unwrap();
    assert_eq!(annots_of(&document), vec![50]);
}

#[test]
fn a_widget_belongs_to_its_field_and_is_refused() {
    let fixture = annotated(
        "",
        &[50],
        &[(
            50,
            b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /Rect [0 0 50 20] >>".as_slice(),
        )],
    );
    let result = delete_annotation(&fixture.open(), 1, id(50));
    assert!(matches!(
        result,
        Err(PdfError::AnnotationNotEditable { ref subtype }) if subtype == "Widget"
    ));
}

#[test]
fn an_annotation_the_page_does_not_list_is_refused() {
    let fixture = annotated("", &[50], &[(50, LINK), (51, LINK)]);
    let result = delete_annotation(&fixture.open(), 1, id(51));
    assert!(matches!(
        result,
        Err(PdfError::AnnotationNotOnPage { page: 1, .. })
    ));
}

#[test]
fn a_page_out_of_range_is_refused() {
    let fixture = annotated("", &[50], &[(50, LINK)]);
    assert!(matches!(
        delete_annotation(&fixture.open(), 3, id(50)),
        Err(PdfError::PageOutOfRange { .. })
    ));
}

#[test]
fn a_structure_element_that_held_only_the_annotation_goes_with_it() {
    // Element 71 holds one object reference (72), to annotation 50, and
    // is the only kid of the root (70). The parent tree maps the key
    // 0 of annotation 50's `StructParent` to element 71.
    let annotation = note("/StructParent 0");
    let fixture = document(
        "",
        "/Annots [50 0 R]",
        "/StructTreeRoot 70 0 R",
        &[
            (50, annotation.as_slice()),
            (
                70,
                b"<< /Type /StructTreeRoot /K 71 0 R /ParentTree 73 0 R >>".as_slice(),
            ),
            (
                71,
                b"<< /Type /StructElem /S /Annot /P 70 0 R /Pg 10 0 R /K 72 0 R >>".as_slice(),
            ),
            (72, b"<< /Type /OBJR /Pg 10 0 R /Obj 50 0 R >>".as_slice()),
            (73, b"<< /Nums [0 71 0 R] >>".as_slice()),
        ],
    );
    let document = delete_annotation(&fixture.open(), 1, id(50)).unwrap();
    let root = dict(&document, 70);
    // The element is no kid of the root any more, and the parent tree has
    // no entry for the annotation's key.
    let kids = document.get(&root, b"K").unwrap();
    let no_kids = matches!(kids.value(), kernkraft_pdf::syntax::Value::Null)
        || kids.array().is_some_and(<[_]>::is_empty);
    assert!(no_kids, "the root still has kids: {:?}", kids.value());
    assert!(
        root.get(b"ParentTree").is_none(),
        "the parent tree still maps the key"
    );
}

#[test]
fn a_structure_element_with_other_content_stays() {
    // Element 71 holds the annotation (72) and a second reference (74), to
    // annotation 51, so it stays and keeps only the second.
    let first = note("/StructParent 0");
    let second = note("/StructParent 1");
    let fixture = document(
        "",
        "/Annots [50 0 R 51 0 R]",
        "/StructTreeRoot 70 0 R",
        &[
            (50, first.as_slice()),
            (51, second.as_slice()),
            (
                70,
                b"<< /Type /StructTreeRoot /K 71 0 R /ParentTree 73 0 R >>".as_slice(),
            ),
            (
                71,
                b"<< /Type /StructElem /S /Annot /P 70 0 R /Pg 10 0 R /K [72 0 R 74 0 R] >>"
                    .as_slice(),
            ),
            (72, b"<< /Type /OBJR /Pg 10 0 R /Obj 50 0 R >>".as_slice()),
            (74, b"<< /Type /OBJR /Pg 10 0 R /Obj 51 0 R >>".as_slice()),
            (73, b"<< /Nums [0 71 0 R 1 71 0 R] >>".as_slice()),
        ],
    );
    let document = delete_annotation(&fixture.open(), 1, id(50)).unwrap();
    let element = dict(&document, 71);
    let kids = document.get(&element, b"K").unwrap();
    let kept: Vec<u32> = kids
        .array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| match item.value() {
                    kernkraft_pdf::syntax::Value::Reference(id) => Some(id.number),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_else(|| match kids.value() {
            kernkraft_pdf::syntax::Value::Reference(id) => vec![id.number],
            other => panic!("K is {other:?}"),
        });
    assert_eq!(kept, vec![74]);
}
