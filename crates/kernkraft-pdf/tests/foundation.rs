// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::*;
use kernkraft_pdf::lopdf::{dictionary, Object};
use kernkraft_pdf::{
    flatten_page_tree, load, load_bytes, page_count, parse_ranges, save, to_bytes, PageRange,
    PdfError,
};

const SAMPLE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../testdata/kern-sample.pdf"
);

fn range(start: u32, end: u32) -> PageRange {
    PageRange { start, end }
}

#[test]
fn loads_sample_and_counts_pages() {
    let doc = load(std::path::Path::new(SAMPLE)).unwrap();
    // The file's root Pages node says /Count 2 with two kids.
    assert_eq!(page_count(&doc), 2);
}

#[test]
fn missing_file_is_fs_error() {
    let dir = tempfile::tempdir().unwrap();
    let err = load(&dir.path().join("none.pdf")).unwrap_err();
    assert!(
        matches!(err, PdfError::Fs(kernkraft_fs::FsError::NotFound { .. })),
        "{err:?}"
    );
}

#[test]
fn garbage_is_parse_error() {
    assert!(matches!(load_bytes(b"not a pdf"), Err(PdfError::Parse(_))));
    assert!(matches!(load_bytes(b""), Err(PdfError::Parse(_))));
}

#[test]
fn truncated_pdf_is_an_error_not_a_panic() {
    let bytes = pdf_n(3);
    for cut in [10, bytes.len() / 3, bytes.len() / 2, bytes.len() - 20] {
        assert!(load_bytes(&bytes[..cut]).is_err(), "cut at {cut}");
    }
}

#[test]
fn encrypted_is_refused() {
    let err = load_bytes(&encrypted_stub()).unwrap_err();
    assert!(matches!(err, PdfError::Encrypted), "{err:?}");
}

#[test]
fn zero_pages_is_refused() {
    let err = load_bytes(&pdf(&[])).unwrap_err();
    assert!(matches!(err, PdfError::NoPages), "{err:?}");
}

#[test]
fn save_round_trips_markers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.pdf");
    let mut doc = load_bytes(&pdf_n(3)).unwrap();
    let stamp = save(&mut doc, &path, None).unwrap();
    assert_eq!(stamp.len, std::fs::metadata(&path).unwrap().len());
    let back = load(&path).unwrap();
    assert_eq!(markers(&back), ["P1", "P2", "P3"]);
}

#[test]
fn save_refuses_externally_changed_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.pdf");
    let mut doc = load_bytes(&pdf_n(1)).unwrap();
    let stamp = save(&mut doc, &path, None).unwrap();
    std::fs::write(&path, b"someone else").unwrap();
    let err = save(&mut doc, &path, Some(&stamp)).unwrap_err();
    assert!(
        matches!(
            err,
            PdfError::Fs(kernkraft_fs::FsError::ExternalModification { .. })
        ),
        "{err:?}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"someone else");
}

#[test]
fn to_bytes_reloads() {
    let mut doc = load_bytes(&pdf_n(2)).unwrap();
    let bytes = to_bytes(&mut doc).unwrap();
    assert_eq!(markers(&load_bytes(&bytes).unwrap()), ["P1", "P2"]);
}

#[test]
fn parse_ranges_accepts_mixed_spec() {
    assert_eq!(
        parse_ranges(" 1-2, 4 ,, ", 5).unwrap(),
        vec![range(1, 2), range(4, 4)]
    );
}

#[test]
fn parse_ranges_rejects_zero() {
    assert!(matches!(
        parse_ranges("0-2", 5),
        Err(PdfError::PageOutOfRange { page: 0, count: 5 })
    ));
}

#[test]
fn parse_ranges_rejects_backwards() {
    assert!(matches!(parse_ranges("3-1", 5), Err(PdfError::BadRange(_))));
}

#[test]
fn parse_ranges_rejects_end_past_last_page() {
    assert!(matches!(
        parse_ranges("2-99", 3),
        Err(PdfError::PageOutOfRange { page: 99, count: 3 })
    ));
}

#[test]
fn parse_ranges_rejects_empty_and_garbage() {
    assert!(matches!(parse_ranges(" , ", 3), Err(PdfError::BadRange(_))));
    assert!(matches!(parse_ranges("a-2", 3), Err(PdfError::BadRange(_))));
    assert!(matches!(
        parse_ranges("1-2-3", 3),
        Err(PdfError::BadRange(_))
    ));
    assert!(matches!(parse_ranges("-2", 3), Err(PdfError::BadRange(_))));
}

#[test]
fn flatten_pushes_inherited_attributes() {
    let mut doc = load_bytes(&nested_pdf()).unwrap();
    flatten_page_tree(&mut doc).unwrap();
    let root = doc
        .catalog()
        .unwrap()
        .get(b"Pages")
        .unwrap()
        .as_reference()
        .unwrap();
    for n in [2, 3] {
        assert_eq!(
            page_attr(&doc, n, b"MediaBox"),
            Some(rect_of(NESTED_MEDIABOX)),
            "page {n}"
        );
        assert!(page_attr(&doc, n, b"Resources").is_some(), "page {n}");
        assert_eq!(
            page_attr(&doc, n, b"Rotate"),
            Some(Object::Integer(NESTED_ROTATE))
        );
    }
    for n in 1..=3 {
        assert_eq!(
            page_attr(&doc, n, b"CropBox"),
            Some(rect_of(NESTED_CROPBOX))
        );
        let id = doc.get_pages()[&n];
        let parent = doc.get_dictionary(id).unwrap().get(b"Parent").unwrap();
        assert_eq!(parent.as_reference().unwrap(), root);
    }
    // Page 1's own MediaBox wins over nothing inherited.
    assert_eq!(
        page_attr(&doc, 1, b"MediaBox"),
        Some(rect_of(PAGE1_MEDIABOX))
    );
    assert_eq!(page_attr(&doc, 1, b"Rotate"), None);
}

#[test]
fn flatten_preserves_order() {
    let mut doc = load_bytes(&nested_pdf()).unwrap();
    assert_eq!(markers(&doc), ["P1", "P2", "P3"]);
    flatten_page_tree(&mut doc).unwrap();
    assert_eq!(markers(&doc), ["P1", "P2", "P3"]);
    let back = load_bytes(&to_bytes(&mut doc).unwrap()).unwrap();
    assert_eq!(markers(&back), ["P1", "P2", "P3"]);
    assert_eq!(page_count(&back), 3);
}

#[test]
fn flatten_drops_intermediate_nodes() {
    let mut doc = load_bytes(&nested_pdf()).unwrap();
    flatten_page_tree(&mut doc).unwrap();
    let pages_nodes = doc
        .objects
        .values()
        .filter(|o| o.type_name().ok() == Some(b"Pages".as_slice()))
        .count();
    assert_eq!(pages_nodes, 1);
}

/// RUSTSEC-2026-0187: a catalog holding an array nested 10,000 deep once
/// overflowed the parser's stack and aborted the process. It must come
/// back as an ordinary result.
#[test]
fn deeply_nested_object_does_not_abort() {
    const DEPTH: usize = 10_000;
    let nested = format!("{}{}", "[".repeat(DEPTH), "]".repeat(DEPTH));
    let body = format!(
        "%PDF-1.7\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R /X {nested} >>\nendobj\n\
         2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n"
    );
    let xref_at = body.len();
    let pdf = format!(
        "{body}xref\n0 3\n0000000000 65535 f \n0000000009 00000 n \n{:010} 00000 n \n\
         trailer\n<< /Size 3 /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
        body.find("2 0 obj").unwrap()
    );
    assert!(load_bytes(pdf.as_bytes()).is_err());
}

/// Root `Pages` id of a fixture being edited.
fn root_of(doc: &kernkraft_pdf::lopdf::Document) -> kernkraft_pdf::lopdf::ObjectId {
    doc.catalog()
        .unwrap()
        .get(b"Pages")
        .unwrap()
        .as_reference()
        .unwrap()
}

/// ISO 32000-1, Table 30 requires `/Type /Page`, but readers meet files
/// without it. MuPDF shows such a kid as a page, and so does the
/// flattening every page operation runs; the page count and the page
/// numbers the operations take must agree with them.
#[test]
fn untyped_page_nodes_count_once() {
    let bytes = edited(&pdf_n(2), |doc, pages| {
        doc.get_dictionary_mut(pages[0]).unwrap().remove(b"Type");
    });
    let mut doc = kernkraft_pdf::lopdf::Document::load_mem(&bytes).unwrap();
    assert_eq!(page_count(&doc), 2, "both kids are pages");
    let first = doc.get_pages()[&1];
    kernkraft_pdf::rotate(&mut doc, &[1], kernkraft_pdf::Rotation::Cw90).unwrap();
    let turned = doc
        .get_dictionary(first)
        .unwrap()
        .get(b"Rotate")
        .ok()
        .cloned();
    assert_eq!(
        turned,
        Some(Object::Integer(90)),
        "page 1 is the page turned"
    );
}

#[test]
fn cyclic_page_tree_is_an_error() {
    let bytes = edited(&pdf_n(1), |doc, _| {
        let root = root_of(doc);
        let dict = doc.get_dictionary_mut(root).unwrap();
        let mut kids = dict.get(b"Kids").unwrap().as_array().unwrap().clone();
        kids.push(Object::Reference(root));
        dict.set("Kids", kids);
    });
    match load_bytes(&bytes) {
        Ok(mut doc) => assert!(matches!(
            flatten_page_tree(&mut doc),
            Err(PdfError::Parse(_))
        )),
        Err(e) => assert!(matches!(e, PdfError::Parse(_) | PdfError::NoPages), "{e:?}"),
    }
}

#[test]
fn page_listed_twice_is_an_error() {
    let bytes = edited(&pdf_n(1), |doc, pages| {
        let root = root_of(doc);
        let dict = doc.get_dictionary_mut(root).unwrap();
        dict.set("Kids", vec![pages[0].into(), pages[0].into()]);
        dict.set("Count", 2);
    });
    let doc = load_bytes(&bytes).unwrap();
    assert!(matches!(
        flatten_page_tree(&mut doc.clone()),
        Err(PdfError::Parse(_))
    ));
    assert!(matches!(
        kernkraft_pdf::extract(&doc, &[1]),
        Err(PdfError::Parse(_))
    ));
}

/// The flattening walks the tree recursively and stops at a fixed depth,
/// so a tree nested past it is refused with a typed error, not a stack
/// overflow.
#[test]
fn deep_tree_is_an_error_not_a_crash() {
    const DEPTH: usize = 300;
    let bytes = edited(&pdf_n(1), |doc, pages| {
        let root = root_of(doc);
        let mut child = pages[0];
        for _ in 0..DEPTH {
            let node = doc.add_object(dictionary! {
                "Type" => "Pages", "Kids" => vec![child.into()], "Count" => 1,
            });
            doc.get_dictionary_mut(child).unwrap().set("Parent", node);
            child = node;
        }
        doc.get_dictionary_mut(child).unwrap().set("Parent", root);
        doc.get_dictionary_mut(root)
            .unwrap()
            .set("Kids", vec![child.into()]);
    });
    let mut doc = load_bytes(&bytes).unwrap();
    assert!(matches!(
        flatten_page_tree(&mut doc),
        Err(PdfError::Parse(_))
    ));
}

#[test]
fn parse_ranges_rejects_malformed_tokens() {
    for spec in ["1-", "-3", "1-2-3", "1--3", "a", "4294967296"] {
        assert!(
            matches!(parse_ranges(spec, 5), Err(PdfError::BadRange(_))),
            "{spec:?}"
        );
    }
}

/// A page number is digits; `u32::from_str` also takes a leading `+`.
#[test]
fn parse_ranges_rejects_plus_sign() {
    assert!(matches!(parse_ranges("+1", 5), Err(PdfError::BadRange(_))));
}

#[test]
fn parse_ranges_keeps_overlap_and_order() {
    assert_eq!(
        parse_ranges("3, 1-2, 2", 3).unwrap(),
        [range(3, 3), range(1, 2), range(2, 2)]
    );
}

#[test]
fn empty_and_header_only_are_parse_errors() {
    for bytes in [&b""[..], b"%PDF-1.7\n"] {
        assert!(
            matches!(load_bytes(bytes), Err(PdfError::Parse(_))),
            "{bytes:?}"
        );
    }
}

#[test]
fn trailing_bytes_after_eof_load() {
    let mut bytes = pdf_n(1);
    bytes.extend_from_slice(b"\njunk");
    assert_eq!(page_count(&load_bytes(&bytes).unwrap()), 1);
}
