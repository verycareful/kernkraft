// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The body of a written file: which objects the trailer reaches, and each
//! of them written in the file (7.3.10, 7.3.8) or kept for an object
//! stream (7.5.7).

use std::collections::{BTreeSet, HashSet};
use std::io::Write as _;
use std::sync::Arc;

use kernkraft_fs::Progress;

use super::{new_id, SECTION_KEYS};
use crate::crypt::{Decryptor, IvSource};
use crate::doc::{Content, DocObject, DocStream, Document, Origin, StreamData};
use crate::file::{Location, XrefEntry};
use crate::filter::flate_encode;
use crate::syntax::{
    write_indirect_object, write_object, Dictionary, Name, Object, ObjectId, Value,
};
use crate::PdfError;

const LENGTH_KEY: &[u8] = b"Length";
const METADATA_KEY: &[u8] = b"Metadata";
const ENCRYPT_KEY: &[u8] = b"Encrypt";

/// 7.5.2: the header line, then a comment of bytes above 127 so that
/// programs that sniff a file's type take it for binary.
const BINARY_COMMENT: &[u8] = b"%\xE2\xE3\xCF\xD3\n";

/// The most objects one written object stream holds. A reader decodes a
/// whole object stream to read one object in it (7.5.7), so the count
/// bounds what reading one object costs; 100 is what qpdf writes.
const OBJECTS_PER_STREAM: usize = 100;

/// Where an object went.
pub(super) enum Placement {
    /// Written in the file at this offset.
    File { offset: u64 },
    /// To go into an object stream: the object's bytes.
    Compressible(Vec<u8>),
}

/// The objects the trailer reaches, by following every reference from
/// the trailer's entries (but those that describe the file's sections),
/// in order of object number. A reference to an object the document does
/// not have is null and leads nowhere.
pub(crate) fn reachable(
    document: &Document,
    trailer: &Dictionary,
    progress: &dyn Progress,
) -> Result<Vec<ObjectId>, PdfError> {
    let mut found = BTreeSet::new();
    let mut seen = HashSet::new();
    let mut pending: Vec<ObjectId> = Vec::new();
    for (key, value) in trailer.iter() {
        if !SECTION_KEYS.contains(&key.as_bytes()) {
            references(value, &mut pending);
        }
    }
    while let Some(id) = pending.pop() {
        if progress.is_cancelled() {
            return Err(PdfError::Cancelled);
        }
        if !seen.insert(id) {
            continue;
        }
        let Some(object) = document.object(id)? else {
            continue;
        };
        found.insert(id);
        match &object.content {
            Content::Object(value) => references(value, &mut pending),
            Content::Stream(stream) => {
                for (_, value) in stream.dictionary.iter() {
                    references(value, &mut pending);
                }
            }
        }
    }
    Ok(found.into_iter().collect())
}

/// Every reference inside `object`.
fn references(object: &Object, out: &mut Vec<ObjectId>) {
    let mut stack = vec![object];
    while let Some(object) = stack.pop() {
        match object.value() {
            Value::Reference(id) => out.push(*id),
            Value::Array(items) => stack.extend(items.iter()),
            Value::Dictionary(dictionary) => stack.extend(dictionary.iter().map(|(_, v)| v)),
            _ => {}
        }
    }
}

/// The bytes of a file being written.
pub(super) struct Writer<'a> {
    pub(super) document: &'a Document,
    pub(super) out: Vec<u8>,
    decryptor: Option<Arc<Decryptor>>,
    ivs: &'a mut IvSource,
    /// The document's metadata stream, which an encrypted file may leave
    /// plain (Table 21, `EncryptMetadata`).
    metadata: Option<ObjectId>,
    /// The encryption dictionary, never encrypted and never in an object
    /// stream (7.5.7).
    encryption_dictionary: Option<ObjectId>,
    /// The next number for an object the writer makes.
    pub(super) next_number: u32,
}

impl<'a> Writer<'a> {
    pub(super) fn new(document: &'a Document, ivs: &'a mut IvSource) -> Result<Self, PdfError> {
        let metadata = match document.catalog() {
            Ok(catalog) => match catalog
                .dictionary()
                .and_then(|catalog| catalog.get(METADATA_KEY))
                .map(Object::value)
            {
                Some(Value::Reference(id)) => Some(*id),
                _ => None,
            },
            Err(_) => None,
        };
        let encryption_dictionary = match document.trailer().get(ENCRYPT_KEY).map(Object::value) {
            Some(Value::Reference(id)) => Some(*id),
            _ => None,
        };
        Ok(Self {
            document,
            out: Vec::new(),
            decryptor: document.decryptor().cloned(),
            ivs,
            metadata,
            encryption_dictionary,
            next_number: document.next_object_number(),
        })
    }

    /// 7.5.2: the version the file's header named.
    pub(super) fn header(&mut self) {
        let version = self.document.file().version();
        let _ = writeln!(self.out, "%PDF-{}.{}", version.major, version.minor);
        self.out.extend_from_slice(BINARY_COMMENT);
    }

    pub(super) fn offset(&self) -> u64 {
        // A vector holds at most isize::MAX bytes, which fits in a u64.
        self.out.len() as u64
    }

    pub(super) fn take_number(&mut self) -> Result<ObjectId, PdfError> {
        let number = self.next_number;
        self.next_number = number
            .checked_add(1)
            .ok_or(PdfError::ObjectNumbersExhausted)?;
        Ok(new_id(number))
    }

    /// Writes `object`, or keeps it for an object stream when `streams`
    /// is set and it may go in one: not a stream, generation 0, not the
    /// encryption dictionary (7.5.7), and not an object that is written as
    /// it lies in the file.
    pub(super) fn place(
        &mut self,
        object: &DocObject,
        streams: bool,
    ) -> Result<Placement, PdfError> {
        let in_file = matches!(
            object.origin,
            Origin::File {
                location: Location::File(_),
                ..
            }
        );
        if let Content::Object(value) = &object.content {
            let compressible = streams
                && !in_file
                && object.id.generation == 0
                && Some(object.id) != self.encryption_dictionary;
            if compressible {
                let bytes = match self.original_compressed_bytes(object)? {
                    Some(bytes) => bytes,
                    None => {
                        let mut bytes = Vec::new();
                        write_object(value, &mut bytes)?;
                        bytes
                    }
                };
                return Ok(Placement::Compressible(bytes));
            }
        }
        let offset = self.offset();
        self.write_in_file(object)?;
        Ok(Placement::File { offset })
    }

    fn write_in_file(&mut self, object: &DocObject) -> Result<(), PdfError> {
        if let Origin::File {
            location: Location::File(span),
            verbatim: true,
        } = object.origin
        {
            let len =
                usize::try_from(span.end - span.start).map_err(|_| PdfError::OutOfBounds {
                    offset: span.start,
                    len: span.end - span.start,
                    size: self.document.file().source().len(),
                })?;
            let bytes = self.document.file().source().read(span.start, len)?;
            self.out.extend_from_slice(&bytes);
            self.out.push(b'\n');
            return Ok(());
        }
        match &object.content {
            Content::Object(value) => {
                if self.decryptor.is_none() {
                    if let Some(bytes) = self.original_compressed_bytes(object)? {
                        self.indirect_bytes(object.id, &bytes);
                        return Ok(());
                    }
                }
                let mut value = value.clone();
                // The encryption dictionary is the one object that is never
                // encrypted (7.6.1).
                if let Some(decryptor) = &self.decryptor {
                    if Some(object.id) != self.encryption_dictionary {
                        decryptor.encrypt_object(object.id, &mut value, self.ivs)?;
                    }
                }
                write_indirect_object(object.id, &value, &mut self.out)?;
                Ok(())
            }
            Content::Stream(stream) => self.write_stream(object.id, stream),
        }
    }

    /// The bytes of `object` in the decoded data of the object stream it
    /// was read from, when it was read without anything tolerated.
    fn original_compressed_bytes(&self, object: &DocObject) -> Result<Option<Vec<u8>>, PdfError> {
        let Origin::File {
            location: Location::ObjectStream { stream, .. },
            verbatim: true,
        } = object.origin
        else {
            return Ok(None);
        };
        let Some(span) = object.object().and_then(Object::span) else {
            return Ok(None);
        };
        let stream = self.document.object_stream(stream)?;
        let data = stream.data()?;
        let range = usize::try_from(span.start)
            .ok()
            .zip(usize::try_from(span.end).ok());
        Ok(range
            .and_then(|(start, end)| data.get(start..end))
            .map(<[u8]>::to_vec))
    }

    /// `N G obj`, `bytes` and `endobj`, each on its own line.
    fn indirect_bytes(&mut self, id: ObjectId, bytes: &[u8]) {
        let _ = writeln!(self.out, "{} {} obj", id.number, id.generation);
        self.out.extend_from_slice(bytes);
        self.out.extend_from_slice(b"\nendobj\n");
    }

    /// A stream written anew: its data as stored (encrypted already when
    /// it comes from an encrypted file; encrypted here when it is new),
    /// its dictionary with a direct `Length`.
    fn write_stream(&mut self, id: ObjectId, stream: &DocStream) -> Result<(), PdfError> {
        let mut dictionary = stream.dictionary.clone();
        let data = match &stream.data {
            StreamData::File(span) => {
                let len =
                    usize::try_from(span.end - span.start).map_err(|_| PdfError::OutOfBounds {
                        offset: span.start,
                        len: span.end - span.start,
                        size: self.document.file().source().len(),
                    })?;
                self.document
                    .file()
                    .source()
                    .read(span.start, len)?
                    .into_owned()
            }
            StreamData::Memory(plain) => match &self.decryptor {
                Some(decryptor) => {
                    decryptor.encrypt_stream(id, &dictionary, plain, self.metadata, self.ivs)?
                }
                None => plain.to_vec(),
            },
        };
        if let Some(decryptor) = &self.decryptor {
            decryptor.encrypt_stream_dictionary(id, &mut dictionary, self.ivs)?;
        }
        self.stream_object(id, dictionary, &data)
    }

    /// Writes stream `id` of `dictionary` and `data`, setting `Length`.
    pub(super) fn stream_object(
        &mut self,
        id: ObjectId,
        mut dictionary: Dictionary,
        data: &[u8],
    ) -> Result<(), PdfError> {
        // A slice holds at most isize::MAX bytes, which fits in an i64.
        dictionary.insert(
            Name::new(LENGTH_KEY),
            Object::new(Value::Integer(data.len() as i64)),
        );
        let _ = writeln!(self.out, "{} {} obj", id.number, id.generation);
        write_object(&Object::new(Value::Dictionary(dictionary)), &mut self.out)?;
        // 7.3.8.1: `stream` is followed by an end-of-line; one before
        // `endstream` is not part of the data.
        self.out.extend_from_slice(b"\nstream\n");
        self.out.extend_from_slice(data);
        self.out.extend_from_slice(b"\nendstream\nendobj\n");
        Ok(())
    }

    /// Writes `objects` into object streams of at most
    /// [`OBJECTS_PER_STREAM`] each (7.5.7), in the order given, and gives
    /// the cross-reference entries of the streams and of the objects.
    pub(super) fn object_streams(
        &mut self,
        objects: Vec<(u32, Vec<u8>)>,
    ) -> Result<Vec<(u32, XrefEntry)>, PdfError> {
        let mut entries =
            Vec::with_capacity(objects.len() + objects.len() / OBJECTS_PER_STREAM + 1);
        for chunk in objects.chunks(OBJECTS_PER_STREAM) {
            let id = self.take_number()?;
            let mut header = Vec::new();
            let mut body = Vec::new();
            for (index, (number, bytes)) in chunk.iter().enumerate() {
                if index > 0 {
                    header.push(b' ');
                }
                let _ = write!(header, "{number} {}", body.len());
                body.extend_from_slice(bytes);
                body.push(b'\n');
                entries.push((
                    *number,
                    XrefEntry::Compressed {
                        stream: id.number,
                        // A chunk holds at most OBJECTS_PER_STREAM objects.
                        index: index as u32,
                    },
                ));
            }
            header.push(b'\n');
            let first = header.len();
            header.extend_from_slice(&body);
            let encoded = flate_encode(&header)?;
            let data = match &self.decryptor {
                Some(decryptor) => {
                    let mut dictionary = Dictionary::new();
                    dictionary.insert(Name::new(b"Type".as_slice()), name(b"ObjStm"));
                    decryptor.encrypt_stream(id, &dictionary, &encoded, None, self.ivs)?
                }
                None => encoded,
            };
            let mut dictionary = Dictionary::new();
            dictionary.insert(Name::new(b"Type".as_slice()), name(b"ObjStm"));
            dictionary.insert(Name::new(b"N".as_slice()), integer(chunk.len()));
            dictionary.insert(Name::new(b"First".as_slice()), integer(first));
            dictionary.insert(Name::new(b"Filter".as_slice()), name(b"FlateDecode"));
            let offset = self.offset();
            self.stream_object(id, dictionary, &data)?;
            entries.push((
                id.number,
                XrefEntry::InUse {
                    offset,
                    generation: 0,
                },
            ));
        }
        Ok(entries)
    }
}

pub(super) fn name(bytes: &[u8]) -> Object {
    Object::new(Value::Name(Name::new(bytes)))
}

pub(super) fn integer(value: usize) -> Object {
    // Every count this writer writes is far below i64::MAX.
    Object::new(Value::Integer(value as i64))
}
