// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The document outline: ISO 32000-1:2008, 12.3.3 (Tables 152 to 154).
//!
//! The items at each level are a list chained by `First` and `Next`; the
//! outline is read along those, and `Last`, `Prev`, `Parent` and the
//! counts, which repeat what the chains say, are not consulted but for
//! the sign of an item's `Count`, which says whether it is open. The
//! items are given as one list in the order a reader shows them, each
//! with its level and its parent's place in the list, so that no part of
//! reading or using an outline goes deeper with each level.
//!
//! An item reached a second time ends the chain it was reached along
//! ([`DiagnosticKind::OutlineLoop`], raised against the item whose
//! `First` or `Next` reached it). An entry that is not what Tables
//! 152 to 154 define is left out of the item and reported
//! ([`DiagnosticKind::IgnoredEntry`]): a `First` or `Next` that is not a
//! reference to a dictionary ends its chain, a `Title` that is not a
//! string is the empty text, a `C` that is not three numbers from 0 to 1
//! is black, and of an `F` with bits past the second only the first two
//! are read.

use std::collections::HashSet;

use crate::action::{Action, ActionKind};
use crate::destination::{Destination, DestinationFile};
use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::doc::{decode_text_string, Document, TextString};
use crate::syntax::{Dictionary, Name, Object, ObjectId, Value};
use crate::PdfError;

const OUTLINES_KEY: &[u8] = b"Outlines";
const FIRST_KEY: &[u8] = b"First";
const NEXT_KEY: &[u8] = b"Next";
const TITLE_KEY: &[u8] = b"Title";
const COUNT_KEY: &[u8] = b"Count";
const DEST_KEY: &[u8] = b"Dest";
const A_KEY: &[u8] = b"A";
const C_KEY: &[u8] = b"C";
const F_KEY: &[u8] = b"F";

/// Table 154: bit 1 italic, bit 2 bold; the others shall be 0.
const ITALIC: i64 = 1 << 0;
const BOLD: i64 = 1 << 1;
/// `C` holds this many components of DeviceRGB, each from 0 to 1.
const RGB_COMPONENTS: usize = 3;

/// One outline item.
#[derive(Debug, Clone, PartialEq)]
pub struct OutlineItem {
    pub id: ObjectId,
    /// 0 for a top-level item, one more for each level below.
    pub level: u32,
    /// The place in the list of the item this one is a child of.
    pub parent: Option<usize>,
    pub title: TextString,
    /// `Dest`, or the destination of the item's go-to action.
    pub destination: Option<Destination>,
    /// `A` and the actions its `Next` chains.
    pub actions: Vec<Action>,
    /// Whether the item shows its children: its `Count` is positive.
    pub open: bool,
    /// `C`: red, green and blue, each from 0 to 1. `None` for the
    /// default, black.
    pub colour: Option<[f64; RGB_COMPONENTS]>,
    pub italic: bool,
    pub bold: bool,
}

/// An item still to be read: its object, level and parent, and the
/// entry it was reached through and the object that entry is in.
struct Pending {
    id: ObjectId,
    level: u32,
    parent: Option<usize>,
    via: &'static [u8],
    from: Option<ObjectId>,
}

impl Document {
    /// The outline's items, in the order a reader shows them; empty when
    /// the document has no outline.
    pub fn outline(&self) -> Result<Vec<OutlineItem>, PdfError> {
        let catalog = self.catalog()?;
        let Some(catalog_dictionary) = catalog.dictionary() else {
            return Ok(Vec::new());
        };
        let root = self.get(catalog_dictionary, OUTLINES_KEY)?;
        let Some(root_dictionary) = root.dictionary() else {
            return Ok(Vec::new());
        };
        let mut diagnostics = Diagnostics::new();
        diagnostics.set_object(root.id());
        let offset = self.offset_of(root.id());
        let mut pending = Vec::new();
        let mut items = Vec::new();
        if let Some(first) = link(root_dictionary, FIRST_KEY, offset, &mut diagnostics) {
            pending.push(Pending {
                id: first,
                level: 0,
                parent: None,
                via: FIRST_KEY,
                from: root.id(),
            });
        }
        let mut seen = HashSet::new();
        while let Some(next) = pending.pop() {
            // A fault in a link is the fault of the item whose entry it is.
            diagnostics.set_object(next.from);
            let from_offset = self.offset_of(next.from);
            if !seen.insert(next.id) {
                diagnostics.push(DiagnosticKind::OutlineLoop, from_offset);
                continue;
            }
            let object = self.object(next.id)?;
            let Some(dictionary) = object
                .as_ref()
                .and_then(|object| object.object().and(object.dictionary()))
            else {
                invalid(&mut diagnostics, next.via, from_offset);
                continue;
            };
            let offset = self.offset_of(Some(next.id));
            diagnostics.set_object(Some(next.id));
            let place = items.len();
            if let Some(sibling) = link(dictionary, NEXT_KEY, offset, &mut diagnostics) {
                pending.push(Pending {
                    id: sibling,
                    level: next.level,
                    parent: next.parent,
                    via: NEXT_KEY,
                    from: Some(next.id),
                });
            }
            if let Some(child) = link(dictionary, FIRST_KEY, offset, &mut diagnostics) {
                pending.push(Pending {
                    id: child,
                    level: next.level.saturating_add(1),
                    parent: Some(place),
                    via: FIRST_KEY,
                    from: Some(next.id),
                });
            }
            items.push(self.outline_item(&next, dictionary, offset, &mut diagnostics)?);
        }
        diagnostics.set_object(None);
        self.record_diagnostics(diagnostics);
        Ok(items)
    }

    fn outline_item(
        &self,
        pending: &Pending,
        dictionary: &Dictionary,
        offset: u64,
        diagnostics: &mut Diagnostics,
    ) -> Result<OutlineItem, PdfError> {
        let title = match self.get(dictionary, TITLE_KEY)?.value() {
            Value::String(title) => decode_text_string(title.as_bytes()),
            _ => {
                invalid(diagnostics, TITLE_KEY, offset);
                decode_text_string(b"")
            }
        };
        let open = match self.get(dictionary, COUNT_KEY)?.value() {
            Value::Integer(count) => *count > 0,
            Value::Null => false,
            _ => {
                invalid(diagnostics, COUNT_KEY, offset);
                false
            }
        };
        let mut destination = match dictionary.get(DEST_KEY) {
            Some(dest) => self.destination(dest, DestinationFile::This, offset, diagnostics)?,
            None => None,
        };
        let actions = match dictionary.get(A_KEY) {
            Some(action) => self.action_sequence(action, diagnostics)?,
            None => Vec::new(),
        };
        if destination.is_none() {
            destination = actions
                .first()
                .filter(|action| action.kind == ActionKind::GoTo)
                .and_then(|action| action.destination);
        }
        let colour = self.colour(dictionary, offset, diagnostics)?;
        let flags = match self.get(dictionary, F_KEY)?.value() {
            Value::Integer(flags) if flags & !(ITALIC | BOLD) == 0 => *flags,
            Value::Integer(flags) => {
                invalid(diagnostics, F_KEY, offset);
                flags & (ITALIC | BOLD)
            }
            Value::Null => 0,
            _ => {
                invalid(diagnostics, F_KEY, offset);
                0
            }
        };
        Ok(OutlineItem {
            id: pending.id,
            level: pending.level,
            parent: pending.parent,
            title,
            destination,
            actions,
            open,
            colour,
            italic: flags & ITALIC != 0,
            bold: flags & BOLD != 0,
        })
    }

    /// `C`, when it is three numbers from 0 to 1.
    fn colour(
        &self,
        dictionary: &Dictionary,
        offset: u64,
        diagnostics: &mut Diagnostics,
    ) -> Result<Option<[f64; RGB_COMPONENTS]>, PdfError> {
        let c = self.get(dictionary, C_KEY)?;
        if matches!(c.value(), Value::Null) {
            return Ok(None);
        }
        let items = c.array().unwrap_or_default();
        let mut colour = [0.0; RGB_COMPONENTS];
        let mut valid = items.len() == RGB_COMPONENTS;
        for (item, slot) in items.iter().zip(&mut colour) {
            *slot = match self.resolve(item)?.value() {
                Value::Integer(integer) => *integer as f64,
                Value::Real(real) => real.get(),
                _ => f64::NAN,
            };
            valid &= (0.0..=1.0).contains(slot);
        }
        if valid {
            Ok(Some(colour))
        } else {
            invalid(diagnostics, C_KEY, offset);
            Ok(None)
        }
    }
}

/// The item `key` names: an indirect reference, as Tables 152 and 153
/// require. Anything else ends the chain and is reported.
fn link(
    dictionary: &Dictionary,
    key: &'static [u8],
    offset: u64,
    diagnostics: &mut Diagnostics,
) -> Option<ObjectId> {
    match dictionary.get(key).map(Object::value) {
        None | Some(Value::Null) => None,
        Some(Value::Reference(id)) => Some(*id),
        Some(_) => {
            invalid(diagnostics, key, offset);
            None
        }
    }
}

fn invalid(diagnostics: &mut Diagnostics, key: &'static [u8], offset: u64) {
    diagnostics.push(
        DiagnosticKind::IgnoredEntry {
            key: Name::new(key),
        },
        offset,
    );
}
