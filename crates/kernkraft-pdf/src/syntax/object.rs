// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The direct objects of ISO 32000-1:2008, 7.3.2 to 7.3.7 and 7.3.9, and
//! the indirect reference of 7.3.10.
//!
//! An object read from a source remembers the span of bytes it was read
//! from. The span is dropped the moment the object, or anything inside it,
//! is changed, so a span always means "these bytes still say exactly this".

use std::borrow::Borrow;
use std::collections::HashMap;

/// A range of bytes in the source an object was read from: `start` is the
/// offset of its first byte and `end` the offset after its last.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: u64,
    pub end: u64,
}

/// The identifier of an indirect object (7.3.10): its object number and its
/// generation number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectId {
    pub number: u32,
    pub generation: u16,
}

/// A name (7.3.5): its bytes with every `#` escape decoded and without the
/// solidus that introduces it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Name(Vec<u8>);

impl Name {
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Self(bytes.into())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

// A name hashes and compares as its bytes, which lets a dictionary be
// searched with a byte slice.
impl Borrow<[u8]> for Name {
    fn borrow(&self) -> &[u8] {
        &self.0
    }
}

/// A string (7.3.4): its bytes as they are after the literal or hexadecimal
/// notation is decoded.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PdfString(Vec<u8>);

impl PdfString {
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Self(bytes.into())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

/// A real number (7.3.3). Always finite: the syntax has no way to write an
/// infinity or a NaN.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Real(f64);

impl Real {
    /// `None` when `value` is infinite or NaN.
    pub fn new(value: f64) -> Option<Self> {
        value.is_finite().then_some(Self(value))
    }

    pub fn get(self) -> f64 {
        self.0
    }

    /// `value` brought into the finite range: an infinity becomes the
    /// largest finite value of its sign. A NaN, which no decimal notation
    /// produces, becomes 0.
    pub(crate) fn saturating(value: f64) -> Self {
        if value.is_nan() {
            Self(0.0)
        } else {
            Self(value.clamp(f64::MIN, f64::MAX))
        }
    }
}

/// What an object is. A stream is not among these: it is never a direct
/// object (7.3.8.1).
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Boolean(bool),
    Integer(i64),
    Real(Real),
    String(PdfString),
    Name(Name),
    Array(Vec<Object>),
    Dictionary(Dictionary),
    Reference(ObjectId),
}

/// A value, and the span it was read from when it was read and has not been
/// changed since.
#[derive(Debug, Clone)]
pub struct Object {
    value: Value,
    span: Option<Span>,
}

impl Object {
    /// An object that was not read from a source.
    pub fn new(value: Value) -> Self {
        Self { value, span: None }
    }

    pub(crate) fn parsed(value: Value, span: Span) -> Self {
        Self {
            value,
            span: Some(span),
        }
    }

    pub fn value(&self) -> &Value {
        &self.value
    }

    /// The value, for changing. The span is dropped, because the bytes it
    /// names may not describe the changed object. An object nested in this
    /// one is reached through this call, so changing it drops the span of
    /// everything that contains it.
    pub fn value_mut(&mut self) -> &mut Value {
        self.span = None;
        &mut self.value
    }

    pub fn into_value(self) -> Value {
        self.value
    }

    /// The bytes the object was read from, when they still describe it.
    pub fn span(&self) -> Option<Span> {
        self.span
    }
}

impl From<Value> for Object {
    fn from(value: Value) -> Self {
        Self::new(value)
    }
}

/// Objects are equal when their values are. Where an object was read from
/// is not part of what it is.
impl PartialEq for Object {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

/// A dictionary (7.3.7). Entries keep the order they were inserted in, so
/// what is read is written back in the order it was found; each key is held
/// once.
#[derive(Debug, Clone, Default)]
pub struct Dictionary {
    entries: Vec<(Name, Object)>,
    /// The position in `entries` of each key, so that finding a key does not
    /// scan the entries.
    index: HashMap<Name, usize>,
}

impl Dictionary {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The value of `key` as the dictionary holds it. A null value is
    /// returned as null: treating it as an absent entry (7.3.7) is left to
    /// the caller, which knows whether it is reading or copying.
    pub fn get(&self, key: &[u8]) -> Option<&Object> {
        let position = *self.index.get(key)?;
        self.entries.get(position).map(|(_, value)| value)
    }

    pub fn get_mut(&mut self, key: &[u8]) -> Option<&mut Object> {
        let position = *self.index.get(key)?;
        self.entries.get_mut(position).map(|(_, value)| value)
    }

    /// Sets the value of `key` and returns the value it had. A key the
    /// dictionary already holds keeps its position; a new key goes last.
    pub fn insert(&mut self, key: Name, value: Object) -> Option<Object> {
        if let Some(entry) = self
            .index
            .get(key.as_bytes())
            .and_then(|&position| self.entries.get_mut(position))
        {
            return Some(std::mem::replace(&mut entry.1, value));
        }
        self.index.insert(key.clone(), self.entries.len());
        self.entries.push((key, value));
        None
    }

    /// Removes `key` and returns its value. The entries after it move up
    /// one position, so this takes time in proportion to their number.
    pub fn remove(&mut self, key: &[u8]) -> Option<Object> {
        let position = self.index.remove(key)?;
        let (_, value) = self.entries.remove(position);
        for (moved, (name, _)) in self.entries.iter().enumerate().skip(position) {
            if let Some(slot) = self.index.get_mut(name.as_bytes()) {
                *slot = moved;
            }
        }
        Some(value)
    }

    /// The entries in the order they were inserted.
    pub fn iter(&self) -> impl Iterator<Item = (&Name, &Object)> {
        self.entries.iter().map(|(key, value)| (key, value))
    }
}

/// Dictionaries are equal when they hold the same entries in the same
/// order.
impl PartialEq for Dictionary {
    fn eq(&self, other: &Self) -> bool {
        self.entries == other.entries
    }
}
