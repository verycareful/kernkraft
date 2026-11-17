// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The reading order a tagged file states: ISO 32000-1:2008, 14.7.2
//! (Table 322: `StructTreeRoot`, `RoleMap`), 14.7.2 (Table 323: structure
//! elements), 14.7.4.2 (Table 324: marked-content references) and 14.8.4
//! (standard structure types, Tables 333 to 340).
//!
//! The structure tree lists the marked-content sequences of a file in
//! reading order. [`StructureIndex`] walks it once, in document order, and
//! records for each sequence (the stream it is in and its `MCID`) its place
//! in that order and the element it belongs to. Text of one block-level
//! element (a paragraph, a heading, a table cell) is kept together by the
//! layout; the order of such blocks follows the tree. An inline-level
//! element (14.8.4.4: a span, a link, a quotation) does not start a block
//! of its own: its text belongs to the element it sits in.

use std::collections::{HashMap, HashSet};

use crate::content::StreamId;
use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::doc::Document;
use crate::error::{Limit, PdfError};
use crate::syntax::limits::MAX_NESTING_DEPTH;
use crate::syntax::{Name, Object, ObjectId, Value};

const STRUCT_TREE_ROOT_KEY: &[u8] = b"StructTreeRoot";
const ROLE_MAP_KEY: &[u8] = b"RoleMap";
const K_KEY: &[u8] = b"K";
const S_KEY: &[u8] = b"S";
const PG_KEY: &[u8] = b"Pg";
const STM_KEY: &[u8] = b"Stm";
const MCID_KEY: &[u8] = b"MCID";
const TYPE_KEY: &[u8] = b"Type";

/// The most steps a `RoleMap` chain is followed: a map that names a type
/// that maps to a third has nothing standard in it past a few, and a cycle
/// has no end.
const MAX_ROLE_STEPS: usize = 16;

/// The inline-level structure types (14.8.4.4, Table 338, and the ruby and
/// warichu types of Table 339).
const INLINE_TYPES: [&[u8]; 15] = [
    b"Span",
    b"Quote",
    b"Note",
    b"Reference",
    b"BibEntry",
    b"Code",
    b"Link",
    b"Annot",
    b"Ruby",
    b"RB",
    b"RT",
    b"RP",
    b"Warichu",
    b"WT",
    b"WP",
];

/// Where a marked-content sequence is in the structure tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Slot {
    /// Its place in the order of the tree: 0 for the first sequence.
    pub order: usize,
    /// The block-level element it belongs to, numbered in document order.
    pub block: usize,
}

/// The marked-content sequences of a file, by stream and `MCID`.
#[derive(Debug, Default)]
pub(crate) struct StructureIndex {
    slots: HashMap<(StreamId, i64), Slot>,
}

impl StructureIndex {
    /// An index of a file with no structure tree.
    pub(crate) fn empty() -> Self {
        Self::default()
    }

    /// The slot of the sequence `mcid` of `stream`.
    pub(crate) fn slot(&self, stream: StreamId, mcid: i64) -> Option<Slot> {
        self.slots.get(&(stream, mcid)).copied()
    }

    /// Walks the structure tree of `doc`, if it has one.
    pub(crate) fn read(doc: &Document, diagnostics: &mut Diagnostics) -> Result<Self, PdfError> {
        let catalog = doc.catalog()?;
        let Some(catalog) = catalog.dictionary() else {
            return Ok(Self::empty());
        };
        let root = doc.get(catalog, STRUCT_TREE_ROOT_KEY)?;
        let Some(root_dictionary) = root.dictionary() else {
            return Ok(Self::empty());
        };
        let roles = Roles::read(doc, root_dictionary)?;
        let mut walk = Walk {
            doc,
            roles,
            diagnostics,
            index: Self::empty(),
            seen: HashSet::new(),
            next_block: 1,
            next_order: 0,
            reported: false,
        };
        if let Some(id) = root.id() {
            walk.seen.insert(id);
        }
        if let Some(kids) = root_dictionary.get(K_KEY) {
            walk.run(
                Node {
                    object: kids.clone(),
                    page: None,
                    block: 0,
                    depth: 1,
                },
                root.id(),
            )?;
        }
        Ok(walk.index)
    }
}

/// `RoleMap`: the standard type a custom one stands for (14.7.3).
struct Roles {
    map: HashMap<Vec<u8>, Vec<u8>>,
}

impl Roles {
    fn read(doc: &Document, root: &crate::syntax::Dictionary) -> Result<Self, PdfError> {
        let mut map = HashMap::new();
        let role_map = doc.get(root, ROLE_MAP_KEY)?;
        if let Some(role_map) = role_map.dictionary() {
            for (custom, standard) in role_map.iter() {
                if let Value::Name(standard) = doc.resolve(standard)?.value() {
                    map.insert(custom.as_bytes().to_vec(), standard.as_bytes().to_vec());
                }
            }
        }
        Ok(Self { map })
    }

    /// The type `name` stands for, after the map has been followed as far
    /// as it goes.
    fn standard<'a>(&'a self, name: &'a [u8]) -> &'a [u8] {
        let mut name = name;
        for _ in 0..MAX_ROLE_STEPS {
            match self.map.get(name) {
                Some(next) => name = next,
                None => break,
            }
        }
        name
    }

    fn is_inline(&self, name: &[u8]) -> bool {
        INLINE_TYPES.contains(&self.standard(name))
    }
}

/// A part of the tree still to be walked: a `K` entry.
struct Node {
    object: Object,
    /// The page the element or an ancestor names, which the sequences
    /// below it are of unless they name another.
    page: Option<ObjectId>,
    /// The block-level element the node's sequences belong to.
    block: usize,
    depth: usize,
}

struct Walk<'a> {
    doc: &'a Document,
    roles: Roles,
    diagnostics: &'a mut Diagnostics,
    index: StructureIndex,
    seen: HashSet<ObjectId>,
    next_block: usize,
    next_order: usize,
    reported: bool,
}

impl Walk<'_> {
    /// Walks `first` and everything below it in document order. `owner`
    /// is the object whose entry `first` is, for the diagnostics.
    fn run(&mut self, first: Node, owner: Option<ObjectId>) -> Result<(), PdfError> {
        let mut stack = vec![first];
        while let Some(node) = stack.pop() {
            if node.depth > MAX_NESTING_DEPTH {
                return Err(PdfError::LimitExceeded {
                    offset: self.doc.offset_of(owner),
                    limit: Limit::NestingDepth,
                });
            }
            let resolved = self.doc.resolve(&node.object)?;
            match resolved.value() {
                Value::Integer(mcid) => self.record(node.page, None, *mcid, node.block, owner),
                Value::Array(items) => {
                    for item in items.iter().rev() {
                        stack.push(Node {
                            object: item.clone(),
                            page: node.page,
                            block: node.block,
                            depth: node.depth + 1,
                        });
                    }
                }
                Value::Dictionary(dictionary) => {
                    let id = resolved.id();
                    if id.is_some_and(|id| !self.seen.insert(id)) {
                        self.invalid_once(K_KEY, id);
                        continue;
                    }
                    // `Pg` is read as written: the reference is the page.
                    let page = match dictionary.get(PG_KEY).map(Object::value) {
                        Some(Value::Reference(page)) => Some(*page),
                        _ => node.page,
                    };
                    let kind = name_bytes(self.doc, dictionary.get(TYPE_KEY))?;
                    let own_type = name_bytes(self.doc, dictionary.get(S_KEY))?;
                    let is_reference = kind.as_deref() == Some(b"MCR".as_slice())
                        || (own_type.is_none()
                            && kind.as_deref() != Some(b"OBJR".as_slice())
                            && dictionary.get(MCID_KEY).is_some());
                    if kind.as_deref() == Some(b"OBJR".as_slice()) {
                        continue;
                    }
                    if is_reference {
                        let mcid = match self.doc.get(dictionary, MCID_KEY)?.value() {
                            Value::Integer(mcid) => Some(*mcid),
                            _ => None,
                        };
                        let stream = match dictionary.get(STM_KEY).map(Object::value) {
                            Some(Value::Reference(stream)) => Some(*stream),
                            _ => None,
                        };
                        match mcid {
                            Some(mcid) => self.record(page, stream, mcid, node.block, id),
                            None => self.invalid_once(MCID_KEY, id),
                        }
                        continue;
                    }
                    let block = match &own_type {
                        Some(name) if self.roles.is_inline(name) => node.block,
                        _ => {
                            let block = self.next_block;
                            self.next_block += 1;
                            block
                        }
                    };
                    if let Some(kids) = dictionary.get(K_KEY) {
                        stack.push(Node {
                            object: kids.clone(),
                            page,
                            block,
                            depth: node.depth + 1,
                        });
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Records the sequence `mcid`, of `stream` when the reference names
    /// one and otherwise of the page content of `page`. The first
    /// reference to a sequence is the one that counts.
    fn record(
        &mut self,
        page: Option<ObjectId>,
        stream: Option<ObjectId>,
        mcid: i64,
        block: usize,
        owner: Option<ObjectId>,
    ) {
        let stream = match (stream, page) {
            (Some(stream), _) => StreamId::Object(stream),
            (None, Some(page)) => StreamId::Page(page),
            (None, None) => {
                self.invalid_once(PG_KEY, owner);
                return;
            }
        };
        let order = self.next_order;
        if let std::collections::hash_map::Entry::Vacant(slot) =
            self.index.slots.entry((stream, mcid))
        {
            slot.insert(Slot { order, block });
            self.next_order += 1;
        }
    }

    /// Reports an entry that is not what Table 323 or 324 defines, once
    /// for the file: the walk goes on without it.
    fn invalid_once(&mut self, key: &'static [u8], object: Option<ObjectId>) {
        if self.reported {
            return;
        }
        self.reported = true;
        self.diagnostics.set_object(object);
        self.diagnostics.push(
            DiagnosticKind::IgnoredEntry {
                key: Name::new(key),
            },
            self.doc.offset_of(object),
        );
        self.diagnostics.set_object(None);
    }
}

/// The bytes of the name `object` is, or the name it refers to.
fn name_bytes(doc: &Document, object: Option<&Object>) -> Result<Option<Vec<u8>>, PdfError> {
    let Some(object) = object else {
        return Ok(None);
    };
    Ok(match doc.resolve(object)?.value() {
        Value::Name(name) => Some(name.as_bytes().to_vec()),
        _ => None,
    })
}
