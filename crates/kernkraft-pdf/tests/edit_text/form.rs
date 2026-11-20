// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Text in form XObjects (ISO 32000-1, 8.10): an edit changes a copy of the
//! form and the `Do` that leads to it, never the form other pages or other
//! invocations share.

use super::*;
use kernkraft_font::FontList;
use kernkraft_pdf::content::StreamId;
use kernkraft_pdf::doc::Document;
use kernkraft_pdf::syntax::{ObjectId, Value};
use kernkraft_pdf::{edit_text, TextEdit, TextStyle};

fn id(number: u32) -> ObjectId {
    ObjectId {
        number,
        generation: 0,
    }
}

const PAGE_RESOURCES: &str = "/Resources << /XObject << /Fm1 70 0 R >> /Font << /F1 60 0 R >> >>";

fn form(data: &str, resources: &str) -> Vec<u8> {
    stream(
        &format!("/Type /XObject /Subtype /Form /BBox [0 0 612 792] {resources}"),
        data.as_bytes(),
    )
}

fn with_form(content: &str, form_data: &str) -> Fixture {
    build(
        content,
        "",
        PAGE_RESOURCES,
        None,
        &[(
            70,
            &form(form_data, "/Resources << /Font << /F1 60 0 R >> >>"),
        )],
    )
}

fn texts(page: &TextPage) -> Vec<String> {
    let mut texts: Vec<String> = page.blocks.iter().map(|b| b.lines[0].text()).collect();
    texts.sort();
    texts
}

fn page_of(doc: &Document) -> TextPage {
    doc.text_page(1, &NoProgress).unwrap()
}

/// The XObject names of the resources of page 1.
fn xobject_names(doc: &Document) -> Vec<String> {
    let page = doc.required(id(PAGE)).unwrap();
    let Some(Value::Dictionary(resources)) = page
        .dictionary()
        .unwrap()
        .get(b"Resources")
        .map(|o| o.value())
    else {
        panic!("no resources of its own");
    };
    let Some(Value::Dictionary(xobjects)) = resources.get(b"XObject").map(|o| o.value()) else {
        panic!("no XObject resources");
    };
    let mut names: Vec<String> = xobjects
        .iter()
        .map(|(name, _)| String::from_utf8(name.as_bytes().to_vec()).unwrap())
        .collect();
    names.sort();
    names
}

#[test]
fn the_form_other_pages_use_is_not_changed() {
    let fixture = with_form("/Fm1 Do", "BT /F1 10 Tf 100 700 Td (Hello) Tj ET");
    let doc = fixture.open();
    let block = page_of(&doc).blocks.remove(0);
    let edited = edit_text(&doc, 1, &block, &TextEdit::new("Hi"), &NoProgress).unwrap();
    assert_eq!(edited.lines, 1);
    // The page shows the copy.
    assert_eq!(texts(&page_of(&edited.document)), ["Hi"]);
    // The form is as it was, and the page invokes a copy of it.
    let old = doc.required(id(70)).unwrap();
    let new = edited.document.required(id(70)).unwrap();
    assert_eq!(old.stream(), new.stream());
    assert_eq!(xobject_names(&edited.document), ["Fm1", "KernForm1"]);
    let content = edited
        .document
        .content_stream(StreamId::Page(id(PAGE)), 1 << 20)
        .unwrap();
    assert_eq!(String::from_utf8(content.data).unwrap(), "/KernForm1 Do");
}

#[test]
fn only_the_invocation_that_was_edited_changes() {
    // The one form is drawn twice, 100 points apart.
    let fixture = with_form(
        "q /Fm1 Do Q q 1 0 0 1 0 -100 cm /Fm1 Do Q",
        "BT /F1 10 Tf 100 700 Td (Hello) Tj ET",
    );
    let doc = fixture.open();
    let before = page_of(&doc);
    assert_eq!(before.blocks.len(), 2);
    // The second drawing is the lower one on the page.
    let lower = before
        .blocks
        .iter()
        .max_by(|a, b| a.rect.top.total_cmp(&b.rect.top))
        .unwrap();
    let edited = edit_text(&doc, 1, lower, &TextEdit::new("Hi"), &NoProgress).unwrap();
    let after = page_of(&edited.document);
    assert_eq!(texts(&after), ["Hello", "Hi"]);
    let hi = after
        .blocks
        .iter()
        .find(|b| b.lines[0].text() == "Hi")
        .unwrap();
    // Hi starts where Hello did and is as tall; it is shorter in width.
    assert_eq!(
        (hi.rect.left, hi.rect.top, hi.rect.bottom),
        (lower.rect.left, lower.rect.top, lower.rect.bottom)
    );
    let hello = after
        .blocks
        .iter()
        .find(|b| b.lines[0].text() == "Hello")
        .unwrap();
    let upper = before
        .blocks
        .iter()
        .min_by(|a, b| a.rect.top.total_cmp(&b.rect.top))
        .unwrap();
    assert_eq!(hello.rect, upper.rect);
}

#[test]
fn a_form_inside_a_form_is_copied_down_the_chain() {
    let objects: Vec<(u32, Vec<u8>)> = vec![
        (
            70,
            form(
                "/Fm2 Do",
                "/Resources << /XObject << /Fm2 71 0 R >> /Font << /F1 60 0 R >> >>",
            ),
        ),
        (
            71,
            form(
                "BT /F1 10 Tf 100 700 Td (Hello) Tj ET",
                "/Resources << /Font << /F1 60 0 R >> >>",
            ),
        ),
    ];
    let borrowed_objects: Vec<(u32, &[u8])> =
        objects.iter().map(|(n, b)| (*n, b.as_slice())).collect();
    let fixture = build("/Fm1 Do", "", PAGE_RESOURCES, None, &borrowed_objects);
    let doc = fixture.open();
    let block = page_of(&doc).blocks.remove(0);
    let edited = edit_text(&doc, 1, &block, &TextEdit::new("Hi"), &NoProgress).unwrap();
    assert_eq!(texts(&page_of(&edited.document)), ["Hi"]);
    for number in [70, 71] {
        assert_eq!(
            doc.required(id(number)).unwrap().stream(),
            edited.document.required(id(number)).unwrap().stream(),
            "form {number}"
        );
    }
}

#[test]
fn a_form_with_no_resources_keeps_the_ones_it_inherits() {
    // The form shows two words in the page's font F1 and has no resources
    // of its own. Setting one in a face from the list gives the copy
    // resources of its own: they must still hold F1 for the other word.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test-sans.ttf");
    std::fs::write(&path, embed_truetype(true)).unwrap();
    let list = FontList::index(&[path], &NoProgress).unwrap();
    let fixture = build(
        "/Fm1 Do",
        "",
        PAGE_RESOURCES,
        None,
        &[(
            70,
            &form(
                "BT /F1 10 Tf 100 700 Td (Hello) Tj 0 -100 Td (World) Tj ET",
                "",
            ),
        )],
    );
    let doc = fixture.open();
    let before = page_of(&doc);
    assert_eq!(before.blocks.len(), 2);
    let edit = TextEdit::new("AB")
        .with_style(TextStyle::default().with_family("Test Sans"))
        .with_fonts(&list);
    let edited = edit_text(&doc, 1, &before.blocks[0], &edit, &NoProgress).unwrap();
    assert_eq!(texts(&page_of(&edited.document)), ["AB", "World"]);
}
