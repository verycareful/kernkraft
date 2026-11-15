// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Edits to a document's objects, made with an [`ObjectBuilder`] and
//! applied with [`Document::apply`].
//!
//! New objects take numbers after the highest the document has, with
//! generation 0 (7.3.10). A new stream's data is held in memory, encoded
//! by the filters its dictionary names.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::{Content, DocObject, DocStream, Document, Edit, Edits, Origin, StreamData};
use crate::filter::flate_encode;
use crate::syntax::{Dictionary, Name, Object, ObjectId, Value};
use crate::PdfError;

const FILTER_KEY: &[u8] = b"Filter";
const DECODE_PARMS_KEY: &[u8] = b"DecodeParms";
const LENGTH_KEY: &[u8] = b"Length";
const FLATE_DECODE: &[u8] = b"FlateDecode";

/// How the data handed to [`ObjectBuilder::add_stream`] is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamEncoding {
    /// Encoded with FlateDecode, which is set as the stream's only filter.
    Flate,
    /// As it is: already encoded by the filters the dictionary names.
    AsGiven,
}

#[derive(Debug, Clone)]
enum Change {
    Set(DocObject),
    Removed(ObjectId),
}

/// Edits ready to apply to the document they were built for.
#[derive(Debug, Clone)]
pub struct Changes {
    /// The next free number of the document the edits were built for,
    /// when they take new numbers.
    built_from: Option<u32>,
    next_number: u32,
    objects: BTreeMap<u32, Change>,
    trailer: Vec<(Name, Option<Object>)>,
}

/// Builds the edits of one step: new objects and streams, objects set or
/// removed, trailer entries set. Used by every operation that writes
/// objects, and public for applications that build their own.
#[derive(Debug, Clone)]
pub struct ObjectBuilder {
    changes: Changes,
}

impl ObjectBuilder {
    /// Edits for `document`.
    pub fn new(document: &Document) -> Self {
        Self {
            changes: Changes {
                built_from: None,
                next_number: document.next_object_number(),
                objects: BTreeMap::new(),
                trailer: Vec::new(),
            },
        }
    }

    /// A new object number, generation 0, for an object set later with
    /// [`Self::set`] or [`Self::set_stream`], so that objects can refer
    /// to each other.
    pub fn reserve(&mut self) -> Result<ObjectId, PdfError> {
        let changes = &mut self.changes;
        let number = changes.next_number;
        changes.next_number = number
            .checked_add(1)
            .ok_or(PdfError::ObjectNumbersExhausted)?;
        changes.built_from.get_or_insert(number);
        Ok(ObjectId {
            number,
            generation: 0,
        })
    }

    /// `count` consecutive new object numbers, generation 0, for objects
    /// set later: the first number is returned. For copying the objects
    /// of another document under numbers that keep their order.
    pub fn reserve_many(&mut self, count: u32) -> Result<u32, PdfError> {
        let changes = &mut self.changes;
        let first = changes.next_number;
        changes.next_number = first
            .checked_add(count)
            .ok_or(PdfError::ObjectNumbersExhausted)?;
        changes.built_from.get_or_insert(first);
        Ok(first)
    }

    /// A new object holding `object`.
    pub fn add(&mut self, object: Object) -> Result<ObjectId, PdfError> {
        let id = self.reserve()?;
        self.set(id, object);
        Ok(id)
    }

    /// A new stream of `dictionary` and `data`.
    pub fn add_stream(
        &mut self,
        dictionary: Dictionary,
        data: Vec<u8>,
        encoding: StreamEncoding,
    ) -> Result<ObjectId, PdfError> {
        let id = self.reserve()?;
        self.set_stream(id, dictionary, data, encoding)?;
        Ok(id)
    }

    /// Object `id` holds `object` from now on.
    pub fn set(&mut self, id: ObjectId, object: Object) {
        self.put(id, Content::Object(object));
    }

    /// Object `id` is a stream of `dictionary` and `data` from now on. Its
    /// `Length` is set to the length of the data as stored.
    pub fn set_stream(
        &mut self,
        id: ObjectId,
        mut dictionary: Dictionary,
        data: Vec<u8>,
        encoding: StreamEncoding,
    ) -> Result<(), PdfError> {
        let data = match encoding {
            StreamEncoding::AsGiven => data,
            StreamEncoding::Flate => {
                dictionary.insert(
                    Name::new(FILTER_KEY),
                    Object::new(Value::Name(Name::new(FLATE_DECODE))),
                );
                dictionary.remove(DECODE_PARMS_KEY);
                flate_encode(&data)?
            }
        };
        // A vector holds at most isize::MAX bytes, which fits in an i64.
        let length = data.len() as i64;
        dictionary.insert(Name::new(LENGTH_KEY), Object::new(Value::Integer(length)));
        self.put(
            id,
            Content::Stream(DocStream {
                dictionary,
                data: StreamData::Memory(Arc::from(data)),
            }),
        );
        Ok(())
    }

    /// Object `id` is removed: a reference to it is null from now on.
    pub fn remove(&mut self, id: ObjectId) {
        self.changes.objects.insert(id.number, Change::Removed(id));
    }

    /// Sets the trailer entry `key`, or removes it when `value` is `None`.
    pub fn set_trailer(&mut self, key: Name, value: Option<Object>) {
        self.changes.trailer.push((key, value));
    }

    pub fn finish(self) -> Changes {
        self.changes
    }

    fn put(&mut self, id: ObjectId, content: Content) {
        self.changes.objects.insert(
            id.number,
            Change::Set(DocObject {
                id,
                content,
                origin: Origin::Edited,
            }),
        );
    }
}

impl Changes {
    /// Whether nothing is changed.
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty() && self.trailer.is_empty()
    }

    /// Applies the changes to the edits of a snapshot. New numbers must
    /// still be free there: changes that take new numbers apply to the
    /// snapshot they were built for, and to no other.
    pub(super) fn apply_to(self, edits: &mut Edits) -> Result<(), PdfError> {
        if let Some(first) = self.built_from {
            if first != edits.next_number {
                return Err(PdfError::Invalid(
                    "the changes were built for another snapshot of the document".to_string(),
                ));
            }
        }
        edits.next_number = edits.next_number.max(self.next_number);
        for (number, change) in self.objects {
            let edit = match change {
                Change::Set(object) => Edit::Set(Arc::new(object)),
                Change::Removed(id) => Edit::Removed(id.generation),
            };
            edits.objects.insert(number, edit);
        }
        for (key, value) in self.trailer {
            match value {
                Some(value) => {
                    edits.trailer.insert(key, value);
                }
                None => {
                    edits.trailer.remove(key.as_bytes());
                }
            }
        }
        Ok(())
    }
}
