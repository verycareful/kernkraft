// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Optional content: ISO 32000-1:2008, 8.11 (optional content), 8.11.2
//! (optional content groups, Table 98), 8.11.2.2 (membership dictionaries,
//! Table 99) and 8.11.4 (configuration, Table 101).
//!
//! The state is the one the default configuration (`OCProperties` / `D`)
//! gives when the document is viewed: `BaseState`, the `ON` and `OFF`
//! arrays, and the usage-application dictionaries (`AS`) of the `View`
//! event. A viewer that lets the user switch groups, or reads another
//! configuration, is not covered; it asks the document for its own
//! [`OptionalContent`] and supplies a different set of switched-off
//! groups.

use std::collections::HashSet;

use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::doc::Document;
use crate::error::PdfError;
use crate::syntax::{Dictionary, Name, Object, ObjectId, Value};

/// How deep a membership expression (`VE`) nests before it is taken as
/// damaged. Table C.1 puts no limit on it; a real expression has two or
/// three levels, and this bounds the work a hostile one asks for.
const MAX_EXPRESSION_DEPTH: usize = 32;

/// The groups of a document that are on and off.
#[derive(Debug, Clone, Default)]
pub struct OptionalContent {
    /// Whether a group the configuration does not name is on.
    base_on: bool,
    on: HashSet<ObjectId>,
    off: HashSet<ObjectId>,
}

impl OptionalContent {
    /// The state of the default configuration for viewing. A document
    /// with no optional content gives every group as on (there are none).
    pub fn read(doc: &Document, diagnostics: &mut Diagnostics) -> Result<Self, PdfError> {
        let mut state = Self {
            base_on: true,
            ..Self::default()
        };
        let catalog = doc.catalog()?;
        let Some(catalog) = catalog.dictionary() else {
            return Ok(state);
        };
        let Some(properties) = entry_dictionary(doc, catalog, b"OCProperties")? else {
            return Ok(state);
        };
        let Some(config) = entry_dictionary(doc, &properties, b"D")? else {
            return Ok(state);
        };
        if let Some(base) = entry_name(doc, &config, b"BaseState")? {
            state.base_on = base.as_bytes() != b"OFF";
        }
        state.on = reference_set(doc, &config, b"ON")?;
        state.off = reference_set(doc, &config, b"OFF")?;
        state.apply_usage(doc, &config, diagnostics)?;
        Ok(state)
    }

    /// Applies the `AS` entries for the `View` event (8.11.4.4): a group
    /// whose usage dictionary gives a `ViewState` for a category the entry
    /// lists is on or off by it.
    fn apply_usage(
        &mut self,
        doc: &Document,
        config: &Dictionary,
        diagnostics: &mut Diagnostics,
    ) -> Result<(), PdfError> {
        let Some(entry) = config.get(b"AS") else {
            return Ok(());
        };
        let resolved = doc.resolve(entry)?;
        let Some(items) = resolved.array() else {
            diagnostics.push(DiagnosticKind::OptionalContentInvalid, 0);
            return Ok(());
        };
        for item in items {
            let resolved = doc.resolve(item)?;
            let Some(usage_app) = resolved.dictionary() else {
                continue;
            };
            if entry_name(doc, usage_app, b"Event")?.is_none_or(|event| event.as_bytes() != b"View")
            {
                continue;
            }
            let lists_view = match usage_app.get(b"Category") {
                None => false,
                Some(categories) => {
                    let categories = doc.resolve(categories)?;
                    categories.array().unwrap_or_default().iter().any(|name| {
                        matches!(name.value(), Value::Name(name) if name.as_bytes() == b"View")
                    })
                }
            };
            if !lists_view {
                continue;
            }
            for group in references(doc, usage_app, b"OCGs")? {
                let Some(group_dictionary) = doc
                    .object(group)?
                    .and_then(|object| object.dictionary().cloned())
                else {
                    continue;
                };
                let Some(usage) = entry_dictionary(doc, &group_dictionary, b"Usage")? else {
                    continue;
                };
                let Some(view) = entry_dictionary(doc, &usage, b"View")? else {
                    continue;
                };
                let Some(view_state) = entry_name(doc, &view, b"ViewState")? else {
                    continue;
                };
                if view_state.as_bytes() == b"OFF" {
                    self.on.remove(&group);
                    self.off.insert(group);
                } else if view_state.as_bytes() == b"ON" {
                    self.off.remove(&group);
                    self.on.insert(group);
                }
            }
        }
        Ok(())
    }

    /// Whether the group `id` is on.
    pub fn group_is_on(&self, id: ObjectId) -> bool {
        if self.off.contains(&id) {
            return false;
        }
        self.on.contains(&id) || self.base_on
    }

    /// Whether the content `object` governs is shown: `object` is the
    /// value of an `OC` entry or of a `Properties` resource, an optional
    /// content group or a membership dictionary (or a reference to one).
    /// A value that is neither is damaged: the content is shown, and
    /// the diagnostic says so.
    pub fn is_visible(
        &self,
        doc: &Document,
        object: &Object,
        diagnostics: &mut Diagnostics,
    ) -> Result<bool, PdfError> {
        let id = match object.value() {
            Value::Reference(id) => Some(*id),
            _ => None,
        };
        let resolved = doc.resolve(object)?;
        let Some(dictionary) = resolved.dictionary() else {
            diagnostics.push(DiagnosticKind::OptionalContentInvalid, 0);
            return Ok(true);
        };
        match entry_name(doc, dictionary, b"Type")?
            .as_ref()
            .map(Name::as_bytes)
        {
            Some(b"OCG") => Ok(id.is_none_or(|id| self.group_is_on(id))),
            Some(b"OCMD") => self.membership(doc, dictionary, diagnostics),
            _ => {
                diagnostics.push(DiagnosticKind::OptionalContentInvalid, 0);
                Ok(true)
            }
        }
    }

    /// An optional content membership dictionary (8.11.2.2): `VE` when it
    /// has one, else the policy `P` over the groups in `OCGs`.
    fn membership(
        &self,
        doc: &Document,
        dictionary: &Dictionary,
        diagnostics: &mut Diagnostics,
    ) -> Result<bool, PdfError> {
        if let Some(expression) = dictionary.get(b"VE") {
            let resolved = doc.resolve(expression)?;
            if let Some(items) = resolved.array() {
                return self.expression(doc, items, 0, diagnostics);
            }
            diagnostics.push(DiagnosticKind::OptionalContentInvalid, 0);
        }
        let groups = references(doc, dictionary, b"OCGs")?;
        if groups.is_empty() {
            return Ok(true);
        }
        let policy = entry_name(doc, dictionary, b"P")?;
        let mut states = groups.iter().map(|group| self.group_is_on(*group));
        Ok(match policy.as_ref().map(Name::as_bytes) {
            Some(b"AllOn") => states.all(|on| on),
            Some(b"AnyOff") => states.any(|on| !on),
            Some(b"AllOff") => states.all(|on| !on),
            // AnyOn is the default.
            _ => states.any(|on| on),
        })
    }

    /// A visibility expression (8.11.2.2): an array of `And`, `Or` or
    /// `Not` and its operands, each a group or another expression.
    fn expression(
        &self,
        doc: &Document,
        items: &[Object],
        depth: usize,
        diagnostics: &mut Diagnostics,
    ) -> Result<bool, PdfError> {
        if depth > MAX_EXPRESSION_DEPTH {
            diagnostics.push(DiagnosticKind::OptionalContentInvalid, 0);
            return Ok(true);
        }
        let Some((operator, operands)) = items.split_first() else {
            diagnostics.push(DiagnosticKind::OptionalContentInvalid, 0);
            return Ok(true);
        };
        let Value::Name(operator) = doc.resolve(operator)?.value().clone() else {
            diagnostics.push(DiagnosticKind::OptionalContentInvalid, 0);
            return Ok(true);
        };
        let mut values = Vec::with_capacity(operands.len());
        for operand in operands {
            let resolved = doc.resolve(operand)?;
            let value = match (operand.value(), resolved.array()) {
                (_, Some(inner)) => self.expression(doc, inner, depth + 1, diagnostics)?,
                (Value::Reference(id), None) => self.group_is_on(*id),
                _ => {
                    diagnostics.push(DiagnosticKind::OptionalContentInvalid, 0);
                    true
                }
            };
            values.push(value);
        }
        Ok(match operator.as_bytes() {
            b"And" => values.iter().all(|value| *value),
            b"Or" => values.iter().any(|value| *value),
            b"Not" => match values.as_slice() {
                [only] => !only,
                _ => {
                    diagnostics.push(DiagnosticKind::OptionalContentInvalid, 0);
                    true
                }
            },
            _ => {
                diagnostics.push(DiagnosticKind::OptionalContentInvalid, 0);
                true
            }
        })
    }
}

fn entry_dictionary(
    doc: &Document,
    dictionary: &Dictionary,
    key: &[u8],
) -> Result<Option<Dictionary>, PdfError> {
    let Some(entry) = dictionary.get(key) else {
        return Ok(None);
    };
    Ok(doc.resolve(entry)?.dictionary().cloned())
}

fn entry_name(
    doc: &Document,
    dictionary: &Dictionary,
    key: &[u8],
) -> Result<Option<Name>, PdfError> {
    let Some(entry) = dictionary.get(key) else {
        return Ok(None);
    };
    Ok(match doc.resolve(entry)?.value() {
        Value::Name(name) => Some(name.clone()),
        _ => None,
    })
}

/// The references an entry holds: one reference, or the references in
/// the array it is.
fn references(
    doc: &Document,
    dictionary: &Dictionary,
    key: &[u8],
) -> Result<Vec<ObjectId>, PdfError> {
    let Some(entry) = dictionary.get(key) else {
        return Ok(Vec::new());
    };
    if let Value::Reference(id) = entry.value() {
        // The reference may name an array of groups, or a group.
        let resolved = doc.resolve(entry)?;
        return Ok(match resolved.array() {
            Some(items) => items.iter().filter_map(reference).collect(),
            None => vec![*id],
        });
    }
    Ok(match entry.value() {
        Value::Array(items) => items.iter().filter_map(reference).collect(),
        _ => Vec::new(),
    })
}

fn reference(object: &Object) -> Option<ObjectId> {
    match object.value() {
        Value::Reference(id) => Some(*id),
        _ => None,
    }
}

fn reference_set(
    doc: &Document,
    dictionary: &Dictionary,
    key: &[u8],
) -> Result<HashSet<ObjectId>, PdfError> {
    Ok(references(doc, dictionary, key)?.into_iter().collect())
}
