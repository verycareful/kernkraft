// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! A document whose objects are numbered from 1 without a gap.
//!
//! The writer keeps object numbers, so what it writes of a few pages of a
//! large document has numbers far above its own size. qpdf takes an object
//! number above about a third of the file's length for impossible and
//! reads such a file as damaged (see [`BYTES_PER_NUMBER`]). A file whose
//! cross-reference is a table is no smaller for it, since the table lists
//! every number, but it is mostly the free list (see [`FREE_PER_OBJECT`]).
//! Either is written a second time from a snapshot that numbers its
//! objects densely, in the order of their old numbers; a file that is
//! sparse is small or mostly free list, so the second pass is cheap.
//!
//! The snapshot holds every object as an edit, so an encrypted file's
//! strings and streams, which are encrypted with a key of the object's
//! number, are decrypted here under the old number and encrypted when
//! written under the new one. A reference to an object the document does
//! not have becomes null (7.3.10).

use std::collections::HashMap;

use crate::doc::{Content, Document, ObjectBuilder, StreamEncoding};
use crate::syntax::{Dictionary, Name, Object, ObjectId, Value};
use crate::PdfError;

/// A file written with an object number above its length divided by this
/// is written again with dense numbers. qpdf 12.4.2 reads a cross-reference
/// stream, or an object stream, numbered above about a third of the file's
/// length as damaged: the smallest file it accepted was 3.0 bytes for each
/// number at 99 and 3.3 at 4233, measured by writing such files by hand.
/// A byte more than the measurements keeps a margin.
pub(super) const BYTES_PER_NUMBER: u64 = 4;

/// A file with more than this many free entries for each object in use
/// is written again with dense numbers: its cross-reference is mostly the
/// free list (20 bytes a number in a table).
pub(super) const FREE_PER_OBJECT: u64 = 8;

/// Whether a file of `len` bytes that writes `count` objects, the highest
/// numbered `highest`, is written again with dense numbers.
pub(super) fn is_sparse(highest: u32, count: usize, len: usize) -> bool {
    let highest = u64::from(highest);
    highest * BYTES_PER_NUMBER > len as u64 || highest > count as u64 * FREE_PER_OBJECT
}

/// A snapshot of `document` in which `ids`, the objects to write, are
/// numbers 1, 2, ... in the order given, generation 0.
pub(super) fn dense(document: &Document, ids: &[ObjectId]) -> Result<Document, PdfError> {
    let mut numbers: HashMap<ObjectId, ObjectId> = HashMap::with_capacity(ids.len());
    for (place, old) in ids.iter().enumerate() {
        let number = u32::try_from(place + 1).map_err(|_| PdfError::ObjectNumbersExhausted)?;
        numbers.insert(
            *old,
            ObjectId {
                number,
                generation: 0,
            },
        );
    }
    let mut builder = ObjectBuilder::new(document);
    for old in ids {
        let object = document.required(*old)?;
        let new = numbers[old];
        match &object.content {
            Content::Object(value) => builder.set(new, remapped(value, &numbers)),
            Content::Stream(stream) => {
                let dictionary = remapped_dictionary(&stream.dictionary, &numbers);
                let data = document.stream_data(*old, stream)?.into_owned();
                builder.set_stream(new, dictionary, data, StreamEncoding::AsGiven)?;
            }
        }
    }
    for (key, value) in document.trailer().iter() {
        builder.set_trailer(key.clone(), Some(remapped(value, &numbers)));
    }
    document.apply(builder.finish())
}

fn remapped(object: &Object, numbers: &HashMap<ObjectId, ObjectId>) -> Object {
    match object.value() {
        Value::Reference(id) => Object::new(match numbers.get(id) {
            Some(new) => Value::Reference(*new),
            None => Value::Null,
        }),
        Value::Array(items) => Object::new(Value::Array(
            items.iter().map(|item| remapped(item, numbers)).collect(),
        )),
        Value::Dictionary(dictionary) => {
            Object::new(Value::Dictionary(remapped_dictionary(dictionary, numbers)))
        }
        _ => object.clone(),
    }
}

fn remapped_dictionary(
    dictionary: &Dictionary,
    numbers: &HashMap<ObjectId, ObjectId>,
) -> Dictionary {
    let mut out = Dictionary::new();
    for (key, value) in dictionary.iter() {
        out.insert(Name::new(key.as_bytes()), remapped(value, numbers));
    }
    out
}
