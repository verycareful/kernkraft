// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The edits of a page operation as they are made.
//!
//! An operation changes many objects, some of them more than once (the
//! catalog is touched by the outline, the destinations, the form and the
//! labels in turn). A [`Rewriter`] holds the current version of every
//! object the operation has set or removed, reads through to the document
//! for the rest, and hands the final versions to an
//! [`ObjectBuilder`](crate::doc::ObjectBuilder) once.
//!
//! An object that is written back unchanged is not an edit: the object
//! stays as the file has it, and is copied as its original bytes.

use std::collections::HashMap;

use crate::doc::{Changes, Document, ObjectBuilder, StreamEncoding};
use crate::syntax::{Dictionary, Name, Object, ObjectId, Value};
use crate::PdfError;

/// The current state of the objects an operation has touched.
pub(crate) struct Rewriter<'d> {
    pub(crate) document: &'d Document,
    builder: ObjectBuilder,
    staged: HashMap<ObjectId, Option<Object>>,
}

impl<'d> Rewriter<'d> {
    pub(crate) fn new(document: &'d Document) -> Self {
        Self {
            document,
            builder: ObjectBuilder::new(document),
            staged: HashMap::new(),
        }
    }

    /// The object `id` as it now stands, or `None` when the document has
    /// no such object, the operation removed it, or it is a stream.
    pub(crate) fn read(&self, id: ObjectId) -> Result<Option<Object>, PdfError> {
        if let Some(staged) = self.staged.get(&id) {
            return Ok(staged.clone());
        }
        Ok(self
            .document
            .object(id)?
            .and_then(|object| object.object().cloned()))
    }

    /// The dictionary object `id` is, as it now stands.
    pub(crate) fn read_dictionary(&self, id: ObjectId) -> Result<Option<Dictionary>, PdfError> {
        Ok(self.read(id)?.and_then(|object| match object.into_value() {
            Value::Dictionary(dictionary) => Some(dictionary),
            _ => None,
        }))
    }

    /// Object `id` holds `object` from now on. Nothing is staged when it
    /// is the object already there.
    pub(crate) fn set(&mut self, id: ObjectId, object: Object) -> Result<(), PdfError> {
        if self.read(id)?.as_ref() == Some(&object) {
            return Ok(());
        }
        self.staged.insert(id, Some(object));
        Ok(())
    }

    /// A new object holding `object`.
    pub(crate) fn add(&mut self, object: Object) -> Result<ObjectId, PdfError> {
        let id = self.builder.reserve()?;
        self.staged.insert(id, Some(object));
        Ok(id)
    }

    /// Runs `edit` on the object `object` stands for: the object itself
    /// when it is direct, the object it refers to when it is a
    /// reference, written back if `edit` changed it. `None` when the
    /// reference leads to no object or to a stream.
    pub(crate) fn follow<R>(
        &mut self,
        object: &mut Object,
        edit: impl FnOnce(&mut Self, &mut Object) -> Result<R, PdfError>,
    ) -> Result<Option<R>, PdfError> {
        let Value::Reference(id) = object.value() else {
            return edit(self, object).map(Some);
        };
        let id = *id;
        let Some(mut target) = self.read(id)? else {
            return Ok(None);
        };
        let result = edit(self, &mut target)?;
        self.set(id, target)?;
        Ok(Some(result))
    }

    /// Runs `edit` on the dictionary of object `id`, written back if it
    /// changed it. `None` when the object is not a dictionary.
    pub(crate) fn edit<R>(
        &mut self,
        id: ObjectId,
        edit: impl FnOnce(&mut Self, &mut Dictionary) -> Result<R, PdfError>,
    ) -> Result<Option<R>, PdfError> {
        let Some(mut object) = self.read(id)? else {
            return Ok(None);
        };
        let Value::Dictionary(dictionary) = object.value_mut() else {
            return Ok(None);
        };
        let result = edit(self, dictionary)?;
        self.set(id, object)?;
        Ok(Some(result))
    }

    /// `count` consecutive new object numbers; the first is returned.
    pub(crate) fn reserve_many(&mut self, count: u32) -> Result<u32, PdfError> {
        self.builder.reserve_many(count)
    }

    /// Object `id` is a stream of `dictionary` and `data`, which is
    /// stored as given.
    pub(crate) fn set_stream(
        &mut self,
        id: ObjectId,
        dictionary: Dictionary,
        data: Vec<u8>,
    ) -> Result<(), PdfError> {
        self.builder
            .set_stream(id, dictionary, data, StreamEncoding::AsGiven)
    }

    /// The stream `id` with `dictionary` as its dictionary and its data as
    /// it is stored (decrypted).
    pub(crate) fn set_stream_dictionary(
        &mut self,
        id: ObjectId,
        dictionary: Dictionary,
    ) -> Result<(), PdfError> {
        let stream = self.document.required(id)?;
        let Some(stream) = stream.stream() else {
            return Err(PdfError::WrongKind { object: id });
        };
        let data = self.document.stream_data(id, stream)?.into_owned();
        self.builder
            .set_stream(id, dictionary, data, StreamEncoding::AsGiven)
    }

    /// The builder the edits are made through, for a stream that is
    /// stored encoded by the builder.
    pub(crate) fn builder(&mut self) -> &mut ObjectBuilder {
        &mut self.builder
    }

    /// The edits.
    pub(crate) fn finish(mut self) -> Changes {
        for (id, object) in self.staged {
            if let Some(object) = object {
                self.builder.set(id, object);
            }
        }
        self.builder.finish()
    }
}

/// The reference `object` is.
pub(crate) fn reference(object: &Object) -> Option<ObjectId> {
    match object.value() {
        Value::Reference(id) => Some(*id),
        _ => None,
    }
}

/// The dictionary `object` is, when it is a direct one.
pub(crate) fn dictionary(object: &Object) -> Option<&Dictionary> {
    match object.value() {
        Value::Dictionary(dictionary) => Some(dictionary),
        _ => None,
    }
}

/// The items of `object`, when it is a direct array.
pub(crate) fn array(object: &Object) -> Option<&[Object]> {
    match object.value() {
        Value::Array(items) => Some(items),
        _ => None,
    }
}

/// The name `object` is.
pub(crate) fn name(object: &Object) -> Option<&[u8]> {
    match object.value() {
        Value::Name(name) => Some(name.as_bytes()),
        _ => None,
    }
}

/// A direct object of `value`.
pub(crate) fn direct(value: Value) -> Object {
    Object::new(value)
}

/// A direct name.
pub(crate) fn name_object(name: &[u8]) -> Object {
    direct(Value::Name(Name::new(name)))
}

/// A direct reference to `id`.
pub(crate) fn reference_object(id: ObjectId) -> Object {
    direct(Value::Reference(id))
}

/// A direct integer.
pub(crate) fn integer(value: i64) -> Object {
    direct(Value::Integer(value))
}
