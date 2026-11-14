// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Tests of the crypt layer. The fixtures of `testdata/encrypted/` are one
//! document, written by `scripts/make-encrypted-fixtures.py`, encrypted
//! by qpdf in every form the standard security handler takes. What the
//! document holds is restated here from that script, and each fixture's
//! encryption from the qpdf options the script gives it.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

mod decryptor;
mod dictionary;
mod fixtures;
mod robustness;

use std::path::PathBuf;
use std::sync::Arc;

use kernkraft_pdf::crypt::{decrypt_file, Decryptor, Encryption};
use kernkraft_pdf::file::{Body, FileObject, PdfFile, ReadLimits};
use kernkraft_pdf::filter::Pipeline;
use kernkraft_pdf::syntax::{Dictionary, Object, ObjectId, Value};
use kernkraft_pdf::{Diagnostics, MemorySource, PdfError};

pub const ROOMY: ReadLimits = ReadLimits {
    max_object_elements: 1 << 16,
    max_xref_entries: 1 << 16,
    max_object_stream_bytes: 1 << 20,
};

/// What `plain.pdf` holds (the constants of the fixture script).
pub const TITLE: &[u8] = b"Kern encrypted fixture";
pub const AUTHOR: &[u8] = &[
    0xFE, 0xFF, 0x00, 0x47, 0x00, 0x72, 0x00, 0x75, 0x03, 0x08, 0x00, 0xDF, 0x00, 0x65,
];
pub const CONTENT: &[u8] = b"BT /F1 12 Tf 72 720 Td (Hello, encrypted world) Tj ET\n";
pub const METADATA: &[u8] = b"<?xpacket begin=\"\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"/></x:xmpmeta>\n\
<?xpacket end=\"w\"?>\n";
pub const ATTACHMENT_NAME: &[u8] = b"note.txt";
pub const ATTACHMENT: &[u8] = b"An attached note.\n";

pub const USER: &str = "user";
pub const OWNER: &str = "owner";
pub const ACCENTED: &str = "p\u{e4}ssw\u{f6}rd\u{20ac}";

pub fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/encrypted")
        .join(format!("{name}.pdf"))
}

pub fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(fixture_path(name)).unwrap()
}

pub fn open_bytes(bytes: Vec<u8>, diagnostics: &mut Diagnostics) -> Result<PdfFile, PdfError> {
    PdfFile::open(Arc::new(MemorySource::new(bytes)), ROOMY, diagnostics)
}

/// An encrypted file opened with a password, ready to read.
pub struct Opened {
    pub file: PdfFile,
    pub decryptor: Arc<Decryptor>,
    pub diagnostics: Diagnostics,
}

/// Opens `bytes` and unlocks it with `password`.
pub fn unlock_bytes(bytes: Vec<u8>, password: &str) -> Result<Opened, PdfError> {
    let mut diagnostics = Diagnostics::new();
    let file = open_bytes(bytes, &mut diagnostics)?;
    let encryption = Encryption::read(&file, &mut diagnostics)?.expect("encrypted");
    let decryptor = Arc::new(encryption.authenticate(password, &mut diagnostics)?);
    let file = decrypt_file(file, decryptor.clone(), &mut diagnostics)?;
    Ok(Opened {
        file,
        decryptor,
        diagnostics,
    })
}

pub fn unlock(name: &str, password: &str) -> Result<Opened, PdfError> {
    unlock_bytes(fixture(name), password)
}

impl Opened {
    /// Object `id`, its strings decrypted.
    pub fn object(&mut self, id: ObjectId) -> FileObject {
        let mut object = self
            .file
            .load(id, &mut self.diagnostics)
            .unwrap()
            .unwrap_or_else(|| panic!("no object {id:?}"));
        self.decryptor
            .decrypt_object(&mut object, &mut self.diagnostics)
            .unwrap();
        object
    }

    /// The dictionary `value` is or refers to.
    pub fn dictionary(&mut self, value: &Value) -> Dictionary {
        match value {
            Value::Dictionary(dictionary) => dictionary.clone(),
            Value::Reference(id) => match self.object(*id).body {
                Body::Object(object) => match object.into_value() {
                    Value::Dictionary(dictionary) => dictionary,
                    other => panic!("not a dictionary: {other:?}"),
                },
                Body::Stream(stream) => stream.dictionary,
            },
            other => panic!("not a dictionary: {other:?}"),
        }
    }

    /// The decrypted, decoded data of stream `id`.
    pub fn stream(&mut self, id: ObjectId) -> Vec<u8> {
        let object = self.object(id);
        let Body::Stream(stream) = object.body else {
            panic!("{id:?} is not a stream");
        };
        let stored = self.file.stream_data(&stream).unwrap().into_owned();
        let encoded = self
            .decryptor
            .decrypt_stream(
                id,
                &stream.dictionary,
                &stored,
                stream.data.start,
                &mut self.diagnostics,
            )
            .unwrap()
            .into_owned();
        Pipeline::from_dictionary(&stream.dictionary)
            .unwrap()
            .decode(&encoded, 1 << 20, stream.data.start, &mut self.diagnostics)
            .unwrap()
            .data
            .into_owned()
    }

    /// What the document holds, read through its object tree.
    pub fn facts(&mut self) -> Facts {
        let trailer = self.file.trailer().clone();
        let info = self.dictionary(get(&trailer, b"Info"));
        let catalog = self.dictionary(get(&trailer, b"Root"));
        let pages = self.dictionary(get(&catalog, b"Pages"));
        let Value::Array(kids) = get(&pages, b"Kids") else {
            panic!("no Kids");
        };
        let page = self.dictionary(kids[0].value());
        let content = self.stream(reference(get(&page, b"Contents")));
        let metadata = self.stream(reference(get(&catalog, b"Metadata")));
        let names = self.dictionary(get(&catalog, b"Names"));
        let files = self.dictionary(get(&names, b"EmbeddedFiles"));
        let Value::Array(pairs) = get(&files, b"Names") else {
            panic!("no Names");
        };
        let attachment_name = string(pairs[0].value());
        let spec = self.dictionary(pairs[1].value());
        let embedded = self.dictionary(get(&spec, b"EF"));
        let attachment = self.stream(reference(get(&embedded, b"F")));
        Facts {
            title: string(get(&info, b"Title")),
            author: string(get(&info, b"Author")),
            content,
            metadata,
            attachment_name,
            attachment,
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Facts {
    pub title: Vec<u8>,
    pub author: Vec<u8>,
    pub content: Vec<u8>,
    pub metadata: Vec<u8>,
    pub attachment_name: Vec<u8>,
    pub attachment: Vec<u8>,
}

/// What every fixture holds once decrypted.
pub fn plain_facts() -> Facts {
    Facts {
        title: TITLE.to_vec(),
        author: AUTHOR.to_vec(),
        content: CONTENT.to_vec(),
        metadata: METADATA.to_vec(),
        attachment_name: ATTACHMENT_NAME.to_vec(),
        attachment: ATTACHMENT.to_vec(),
    }
}

pub fn get<'a>(dictionary: &'a Dictionary, key: &[u8]) -> &'a Value {
    dictionary
        .get(key)
        .map(Object::value)
        .unwrap_or_else(|| panic!("no {}", String::from_utf8_lossy(key)))
}

pub fn reference(value: &Value) -> ObjectId {
    match value {
        Value::Reference(id) => *id,
        other => panic!("not a reference: {other:?}"),
    }
}

pub fn string(value: &Value) -> Vec<u8> {
    match value {
        Value::String(string) => string.as_bytes().to_vec(),
        other => panic!("not a string: {other:?}"),
    }
}

/// A file of one catalog whose trailer holds `trailer_entries` too, its
/// offsets right, for encryption dictionaries a test writes itself.
pub fn file_with_trailer(objects: &[&str], trailer_entries: &str) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    let mut bodies = vec!["<</Type /Catalog>>"];
    bodies.extend_from_slice(objects);
    for (number, body) in (1..).zip(&bodies) {
        offsets.push(out.len());
        out.extend(format!("{number} 0 obj\n{body}\nendobj\n").bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", bodies.len() + 1).bytes());
    for offset in offsets {
        out.extend(format!("{offset:010} 00000 n \n").bytes());
    }
    out.extend(
        format!(
            "trailer\n<</Size {} /Root 1 0 R {trailer_entries}>>\nstartxref\n{xref}\n%%EOF\n",
            bodies.len() + 1
        )
        .bytes(),
    );
    out
}
