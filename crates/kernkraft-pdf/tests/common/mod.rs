// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Fixture PDFs built in memory, so every expectation is derived from what
//! the fixture contains. Page n of a fixture draws the text `P<n>` with an
//! uncompressed content stream, which `markers` reads back.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use kernkraft_pdf::lopdf::{dictionary, Document, Object, ObjectId, Stream, StringFormat};

pub const NESTED_MEDIABOX: [i64; 4] = [0, 0, 200, 300];
pub const NESTED_CROPBOX: [i64; 4] = [0, 0, 150, 150];
pub const NESTED_ROTATE: i64 = 90;
pub const PAGE1_MEDIABOX: [i64; 4] = [0, 0, 100, 100];

fn font(doc: &mut Document) -> ObjectId {
    doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    })
}

fn resources(doc: &mut Document, font_id: ObjectId) -> ObjectId {
    doc.add_object(dictionary! { "Font" => dictionary! { "F1" => font_id } })
}

fn content(doc: &mut Document, n: u32) -> ObjectId {
    let ops = format!("BT /F1 12 Tf 10 10 Td (P{n}) Tj ET");
    doc.add_object(Stream::new(dictionary! {}, ops.into_bytes()))
}

fn rect(r: [i64; 4]) -> Object {
    Object::Array(r.iter().map(|&v| Object::Integer(v)).collect())
}

fn finish(mut doc: Document, pages_id: ObjectId, extra_catalog: &[(&str, Object)]) -> Vec<u8> {
    let mut catalog = dictionary! { "Type" => "Catalog", "Pages" => pages_id };
    for (k, v) in extra_catalog {
        catalog.set(*k, v.clone());
    }
    let catalog_id = doc.add_object(catalog);
    doc.trailer.set("Root", catalog_id);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

/// A flat document: page n has MediaBox `[0 0 w h]` and its own Resources.
pub fn pdf(pages: &[(f32, f32)]) -> Vec<u8> {
    let (doc, pages_id, _) = flat(pages);
    finish(doc, pages_id, &[])
}

fn flat(pages: &[(f32, f32)]) -> (Document, ObjectId, Vec<ObjectId>) {
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let font_id = font(&mut doc);
    let res = resources(&mut doc, font_id);
    let mut kids = Vec::new();
    for (i, &(w, h)) in pages.iter().enumerate() {
        let c = content(&mut doc, i as u32 + 1);
        let page = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), w.into(), h.into()],
            "Resources" => res,
            "Contents" => c,
        });
        kids.push(page);
    }
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Count" => kids.len() as i64,
            "Kids" => kids.iter().map(|&k| Object::Reference(k)).collect::<Vec<_>>(),
        }),
    );
    (doc, pages_id, kids)
}

/// `n` Letter-sized pages.
pub fn pdf_n(n: u32) -> Vec<u8> {
    pdf(&vec![(612.0, 792.0); n as usize])
}

/// A nested page tree:
///
/// ```text
/// root Pages (CropBox NESTED_CROPBOX)
///   page 1 (own MediaBox PAGE1_MEDIABOX, own Resources)
///   intermediate Pages (MediaBox NESTED_MEDIABOX, Resources, Rotate 90)
///     page 2 (nothing of its own)
///     page 3 (nothing of its own)
/// ```
pub fn nested_pdf() -> Vec<u8> {
    let mut doc = Document::with_version("1.7");
    let root = doc.new_object_id();
    let mid = doc.new_object_id();
    let font_id = font(&mut doc);
    let res = resources(&mut doc, font_id);
    let c1 = content(&mut doc, 1);
    let p1 = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => root, "MediaBox" => rect(PAGE1_MEDIABOX),
        "Resources" => res, "Contents" => c1,
    });
    let c2 = content(&mut doc, 2);
    let p2 = doc.add_object(dictionary! { "Type" => "Page", "Parent" => mid, "Contents" => c2 });
    let c3 = content(&mut doc, 3);
    let p3 = doc.add_object(dictionary! { "Type" => "Page", "Parent" => mid, "Contents" => c3 });
    doc.objects.insert(
        mid,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Parent" => root, "Count" => 2,
            "Kids" => vec![p2.into(), p3.into()],
            "MediaBox" => rect(NESTED_MEDIABOX), "Resources" => res, "Rotate" => NESTED_ROTATE,
        }),
    );
    doc.objects.insert(
        root,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Count" => 3, "Kids" => vec![p1.into(), mid.into()],
            "CropBox" => rect(NESTED_CROPBOX),
        }),
    );
    finish(doc, root, &[])
}

/// `pages` Letter pages with a flat outline: each `(page, title)` is a
/// top-level bookmark to that 1-based page, through a `/Dest` array.
pub fn with_outline(pages: u32, marks: &[(u32, &str)]) -> Vec<u8> {
    let (mut doc, pages_id, kids) = flat(&vec![(612.0, 792.0); pages as usize]);
    let outlines = doc.new_object_id();
    let ids: Vec<ObjectId> = marks.iter().map(|_| doc.new_object_id()).collect();
    for (i, &(page, title)) in marks.iter().enumerate() {
        let mut item = dictionary! {
            "Title" => Object::String(title.as_bytes().to_vec(), StringFormat::Literal),
            "Parent" => outlines,
            "Dest" => vec![kids[page as usize - 1].into(), "Fit".into()],
        };
        if i > 0 {
            item.set("Prev", ids[i - 1]);
        }
        if i + 1 < ids.len() {
            item.set("Next", ids[i + 1]);
        }
        doc.objects.insert(ids[i], Object::Dictionary(item));
    }
    let mut dict = dictionary! { "Type" => "Outlines", "Count" => ids.len() as i64 };
    if let (Some(first), Some(last)) = (ids.first(), ids.last()) {
        dict.set("First", *first);
        dict.set("Last", *last);
    }
    doc.objects.insert(outlines, Object::Dictionary(dict));
    finish(doc, pages_id, &[("Outlines", Object::Reference(outlines))])
}

/// The `P<n>` marker of each page, in page order.
pub fn markers(doc: &Document) -> Vec<String> {
    doc.get_pages()
        .values()
        .map(|&id| {
            let bytes = doc.get_page_content(id);
            let text = String::from_utf8_lossy(&bytes);
            let start = text.find("(P").unwrap() + 1;
            let end = start + text[start..].find(')').unwrap();
            text[start..end].to_string()
        })
        .collect()
}

/// A one-page file whose trailer carries a standard security handler
/// `/Encrypt` dictionary that the empty password does not open.
pub fn encrypted_stub() -> Vec<u8> {
    let (mut doc, pages_id, _) = flat(&[(612.0, 792.0)]);
    let encrypt = doc.add_object(dictionary! {
        "Filter" => "Standard",
        "V" => 1,
        "R" => 2,
        "O" => Object::String(vec![0x11; 32], StringFormat::Hexadecimal),
        "U" => Object::String(vec![0x22; 32], StringFormat::Hexadecimal),
        "P" => -4,
    });
    doc.trailer.set("Encrypt", encrypt);
    doc.trailer.set(
        "ID",
        vec![
            Object::String(vec![0x33; 16], StringFormat::Hexadecimal),
            Object::String(vec![0x33; 16], StringFormat::Hexadecimal),
        ],
    );
    finish(doc, pages_id, &[])
}

/// The value of `key` on the page dictionary of 1-based page `n`, followed
/// through one reference.
pub fn page_attr(doc: &Document, n: u32, key: &[u8]) -> Option<Object> {
    let id = doc.get_pages()[&n];
    let dict = doc.get_dictionary(id).unwrap();
    let value = dict.get(key).ok()?;
    match value {
        Object::Reference(r) => doc.get_object(*r).ok().cloned(),
        other => Some(other.clone()),
    }
}

pub fn rect_of(r: [i64; 4]) -> Object {
    rect(r)
}

/// Edits the catalog of a fixture: `edit` gets the document and the page
/// ids in order, and may add objects and catalog entries.
pub fn edited(bytes: &[u8], edit: impl FnOnce(&mut Document, &[ObjectId])) -> Vec<u8> {
    let mut doc = Document::load_mem(bytes).unwrap();
    let pages: Vec<ObjectId> = doc.get_pages().into_values().collect();
    edit(&mut doc, &pages);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

pub fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}
