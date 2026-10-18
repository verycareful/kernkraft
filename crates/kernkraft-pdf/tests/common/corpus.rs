// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The round-trip corpus: one fixture per file structure kernkraft must
//! read and write back. Each is built in memory (except `kern-sample`), so
//! its page count and markers follow from how it was built.

use std::collections::HashSet;
use std::io::Write;

use kernkraft_pdf::lopdf::{dictionary, Document, Object, ObjectId, Stream};

use super::{edited, pdf_n};

/// The sample shipped in `testdata/`.
pub const SAMPLE: &[u8] = include_bytes!("../../../../testdata/kern-sample.pdf");

/// Inheritable page attributes (ISO 32000-1:2008, Table 30).
pub const INHERITABLE: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

/// Bound on `/Parent` links followed, so a cyclic fixture cannot hang a test.
const MAX_PARENTS: usize = 64;

pub struct Fixture {
    pub name: &'static str,
    pub bytes: Vec<u8>,
    pub pages: u32,
    /// `P<n>` markers in page order; `None` for a file without them.
    pub markers: Option<Vec<String>>,
}

fn p_markers(ns: &[u32]) -> Option<Vec<String>> {
    Some(ns.iter().map(|n| format!("P{n}")).collect())
}

fn fixture(name: &'static str, bytes: Vec<u8>, ns: &[u32]) -> Fixture {
    Fixture {
        name,
        bytes,
        pages: ns.len() as u32,
        markers: p_markers(ns),
    }
}

pub fn corpus() -> Vec<Fixture> {
    vec![
        fixture("classic", pdf_n(3), &[1, 2, 3]),
        fixture("nested", super::nested_pdf(), &[1, 2, 3]),
        fixture("object-streams", object_streams(4), &[1, 2, 3, 4]),
        fixture("flate", flate(3), &[1, 2, 3]),
        fixture("incremental", incremental(), &[1, 9]),
        fixture("contents-array", contents_array(), &[1]),
        fixture("indirect-kids", indirect_kids(3), &[1, 2, 3]),
        fixture("shared-image", shared_image(), &[1, 2]),
        fixture("info", with_info(2), &[1, 2]),
        Fixture {
            name: "kern-sample",
            bytes: SAMPLE.to_vec(),
            pages: raw_page_objects(SAMPLE),
            markers: None,
        },
    ]
}

/// Written with an xref stream and object streams (ISO 32000-1, 7.5.7, 7.5.8).
pub fn object_streams(n: u32) -> Vec<u8> {
    let mut doc = Document::load_mem(&pdf_n(n)).unwrap();
    let mut out = Vec::new();
    doc.save_modern(&mut out).unwrap();
    out
}

/// Every stream Flate-compressed.
pub fn flate(n: u32) -> Vec<u8> {
    let mut doc = Document::load_mem(&pdf_n(n)).unwrap();
    doc.compress();
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

/// `pdf_n(2)` plus an incremental update (ISO 32000-1, 7.5.6) that gives
/// page 2 a new content stream drawing `P9`.
pub fn incremental() -> Vec<u8> {
    let mut bytes = pdf_n(2);
    let doc = Document::load_mem(&bytes).unwrap();
    let page2 = doc.get_pages()[&2];
    let page = doc.get_dictionary(page2).unwrap();
    let parent = page.get(b"Parent").unwrap().as_reference().unwrap();
    let resources = page.get(b"Resources").unwrap().as_reference().unwrap();
    let root = doc.trailer.get(b"Root").unwrap().as_reference().unwrap();
    let prev = startxref(&bytes);
    let content_id = doc.max_id + 1;

    if !bytes.ends_with(b"\n") {
        bytes.push(b'\n');
    }
    let ops = b"BT /F1 12 Tf 10 10 Td (P9) Tj ET";
    let content_at = bytes.len();
    write!(
        bytes,
        "{content_id} 0 obj\n<< /Length {} >>\nstream\n",
        ops.len()
    )
    .unwrap();
    bytes.extend_from_slice(ops);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    let page_at = bytes.len();
    write!(
        bytes,
        "{} 0 obj\n<< /Type /Page /Parent {} 0 R /MediaBox [0 0 612 792] /Resources {} 0 R /Contents {content_id} 0 R >>\nendobj\n",
        page2.0, parent.0, resources.0
    )
    .unwrap();
    let xref_at = bytes.len();
    // Each xref entry is exactly 20 bytes (7.5.4).
    write!(
        bytes,
        "xref\n{} 1\n{page_at:010} 00000 n\r\n{content_id} 1\n{content_at:010} 00000 n\r\n",
        page2.0
    )
    .unwrap();
    write!(
        bytes,
        "trailer\n<< /Size {} /Root {} 0 R /Prev {prev} >>\nstartxref\n{xref_at}\n%%EOF\n",
        content_id + 1,
        root.0
    )
    .unwrap();
    bytes
}

/// The byte offset after the file's last `startxref`.
fn startxref(bytes: &[u8]) -> usize {
    let key = b"startxref";
    let at = bytes.windows(key.len()).rposition(|w| w == key).unwrap() + key.len();
    let digits: String = bytes[at..]
        .iter()
        .map(|&b| b as char)
        .skip_while(|c| c.is_whitespace())
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().unwrap()
}

/// One page whose `/Contents` is an array of two streams, split at a token
/// boundary as 7.8.2 requires.
pub fn contents_array() -> Vec<u8> {
    edited(&pdf_n(1), |doc, pages| {
        let a = doc.add_object(Stream::new(
            dictionary! {},
            b"BT /F1 12 Tf 10 10 Td".to_vec(),
        ));
        let b = doc.add_object(Stream::new(dictionary! {}, b"(P1) Tj ET".to_vec()));
        doc.get_dictionary_mut(pages[0])
            .unwrap()
            .set("Contents", vec![a.into(), b.into()]);
    })
}

/// The root `/Kids` is an indirect reference to the array.
pub fn indirect_kids(n: u32) -> Vec<u8> {
    edited(&pdf_n(n), |doc, _| {
        let root = root_pages(doc);
        let kids = doc
            .get_dictionary(root)
            .unwrap()
            .get(b"Kids")
            .unwrap()
            .clone();
        let kids_id = doc.add_object(kids);
        doc.get_dictionary_mut(root)
            .unwrap()
            .set("Kids", Object::Reference(kids_id));
    })
}

/// Name of the image both pages of [`shared_image`] draw.
pub const SHARED_IMAGE: &[u8] = b"Im1";

/// Two pages, each with its own `/Resources`, both drawing one 1 x 1 image.
pub fn shared_image() -> Vec<u8> {
    edited(&pdf_n(2), |doc, pages| {
        let image = doc.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject", "Subtype" => "Image", "Width" => 1, "Height" => 1,
                "ColorSpace" => "DeviceGray", "BitsPerComponent" => 8,
            },
            vec![0],
        ));
        let font = doc
            .get_object(
                doc.get_dictionary(pages[0])
                    .unwrap()
                    .get(b"Resources")
                    .unwrap()
                    .as_reference()
                    .unwrap(),
            )
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"Font")
            .unwrap()
            .clone();
        for &page in pages {
            let mut xobjects = kernkraft_pdf::lopdf::Dictionary::new();
            xobjects.set(SHARED_IMAGE.to_vec(), Object::Reference(image));
            let res = doc.add_object(dictionary! { "Font" => font.clone(), "XObject" => xobjects });
            let dict = doc.get_dictionary_mut(page).unwrap();
            dict.set("Resources", res);
            let content = dict.get(b"Contents").unwrap().as_reference().unwrap();
            let stream = doc
                .get_object_mut(content)
                .unwrap()
                .as_stream_mut()
                .unwrap();
            let mut ops = stream.content.clone();
            ops.extend_from_slice(b"\nq 10 0 0 10 50 50 cm /Im1 Do Q");
            stream.set_content(ops);
        }
    })
}

/// Title written to the `/Info` dictionary of [`with_info`].
pub const INFO_TITLE: &str = "Corpus";

pub fn with_info(n: u32) -> Vec<u8> {
    edited(&pdf_n(n), |doc, _| {
        let info = doc.add_object(dictionary! {
            "Title" => Object::string_literal(INFO_TITLE),
        });
        doc.trailer.set("Info", info);
    })
}

/// Count of `/Type /Page` (not `/Pages`) in a file whose page objects are
/// stored uncompressed, read from the bytes rather than through a parser.
pub fn raw_page_objects(bytes: &[u8]) -> u32 {
    let key = b"/Type /Page";
    bytes
        .windows(key.len() + 1)
        .filter(|w| &w[..key.len()] == key && w[key.len()] != b's')
        .count() as u32
}

pub fn root_pages(doc: &Document) -> ObjectId {
    doc.catalog()
        .unwrap()
        .get(b"Pages")
        .unwrap()
        .as_reference()
        .unwrap()
}

/// The value of an inheritable `key` for 1-based page `n`: the page's own,
/// else the nearest ancestor's (7.7.3.4), one reference followed.
pub fn effective(doc: &Document, n: u32, key: &[u8]) -> Option<Object> {
    let mut id = doc.get_pages()[&n];
    for _ in 0..MAX_PARENTS {
        let dict = doc.get_dictionary(id).ok()?;
        if let Ok(value) = dict.get(key) {
            return Some(match value {
                Object::Reference(r) => doc.get_object(*r).ok()?.clone(),
                other => other.clone(),
            });
        }
        id = dict.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

/// Every inheritable attribute of every page, resolved.
pub fn page_attrs(doc: &Document) -> Vec<Vec<Option<Object>>> {
    (1..=doc.get_pages().len() as u32)
        .map(|n| INHERITABLE.iter().map(|k| effective(doc, n, k)).collect())
        .collect()
}

/// Panics if any reference in the objects or the trailer names an object
/// the document does not hold.
pub fn assert_no_dangling(doc: &Document, what: &str) {
    let held: HashSet<ObjectId> = doc.objects.keys().copied().collect();
    let mut stack: Vec<&Object> = doc.objects.values().collect();
    stack.extend(doc.trailer.iter().map(|(_, v)| v));
    while let Some(o) = stack.pop() {
        match o {
            Object::Reference(id) => assert!(held.contains(id), "{what}: dangling {id:?}"),
            Object::Array(items) => stack.extend(items.iter()),
            Object::Dictionary(d) => stack.extend(d.iter().map(|(_, v)| v)),
            Object::Stream(s) => stack.extend(s.dict.iter().map(|(_, v)| v)),
            _ => {}
        }
    }
}
