// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! A report of what in a document would run code, reach outside it or
//! send data, were a viewer to act on it: ISO 32000-1:2008, 12.6.4 (action
//! types), 12.7.5 (form actions), 12.7.8 (XFA) and 7.11.4 (embedded
//! files). Nothing reported is ever performed.
//!
//! Every object the trailer reaches is read, and every action dictionary
//! in it is looked at, wherever it hangs: an `OpenAction`, an `A` or `AA`
//! of the catalog, a page, an annotation or a field, an outline item, the
//! `JavaScript` name tree, a `Next`. Reading the objects rather than the
//! places that trigger actions means no trigger can be missed, and an
//! action nothing reaches is not reported, since nothing can perform it.
//!
//! An action is reported when it carries a JavaScript script, launches an
//! application, goes to another file or one embedded in this one, sends
//! form data, imports it, or opens a URI.

use std::collections::HashMap;

use kernkraft_fs::Progress;

use crate::action::{Action, ActionKind};
use crate::diagnostic::Diagnostics;
use crate::doc::{Content, Document};
use crate::syntax::{Dictionary, Object, ObjectId, Value};
use crate::write::reachable;
use crate::PdfError;

const TYPE_KEY: &[u8] = b"Type";
const ACTION_TYPE: &[u8] = b"Action";
const S_KEY: &[u8] = b"S";
const ANNOTS_KEY: &[u8] = b"Annots";
const ACRO_FORM_KEY: &[u8] = b"AcroForm";
const XFA_KEY: &[u8] = b"XFA";

/// An action the report names, and where it is.
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveAction {
    pub action: Action,
    /// The indirect object the action dictionary is, or is inside.
    pub holder: ObjectId,
    /// The page the holder is, or whose annotation it is.
    pub page: Option<u32>,
}

/// What a document holds that a viewer could run or follow out of it.
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveContent {
    pub actions: Vec<ActiveAction>,
    /// Whether the interactive form has XFA data (12.7.8), which can
    /// carry scripts of its own.
    pub xfa: bool,
    /// How many files the document embeds ([`Document::attachments`]).
    pub attachments: usize,
}

impl ActiveContent {
    /// Whether the report names nothing.
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty() && !self.xfa && self.attachments == 0
    }
}

/// Whether `action` is one the report names.
fn is_active(action: &Action) -> bool {
    action.script
        || matches!(
            action.kind,
            ActionKind::Launch
                | ActionKind::GoToRemote
                | ActionKind::GoToEmbedded
                | ActionKind::SubmitForm
                | ActionKind::ImportData
                | ActionKind::Uri
        )
}

/// Whether `dictionary` is an action dictionary (Table 193): `S` is a name
/// and `Type`, when present, is `Action`.
fn is_action(dictionary: &Dictionary) -> bool {
    let typed = match dictionary.get(TYPE_KEY).map(Object::value) {
        None => true,
        Some(Value::Name(name)) => name.as_bytes() == ACTION_TYPE,
        Some(_) => false,
    };
    typed
        && matches!(
            dictionary.get(S_KEY).map(Object::value),
            Some(Value::Name(_))
        )
}

impl Document {
    /// The report of this document. `progress` counts the objects read; a
    /// cancelled report is [`PdfError::Cancelled`].
    pub fn active_content(&self, progress: &dyn Progress) -> Result<ActiveContent, PdfError> {
        let numbers = reachable(self, self.trailer(), progress)?;
        let pages = self.pages_of_holders()?;
        let mut diagnostics = Diagnostics::new();
        let mut actions = Vec::new();
        let total = numbers.len() as u64;
        for (done, id) in numbers.iter().enumerate() {
            if progress.is_cancelled() {
                return Err(PdfError::Cancelled);
            }
            progress.report(done as u64, total, "objects");
            let Some(object) = self.object(*id)? else {
                continue;
            };
            let (top, nested): (Option<&Dictionary>, Vec<&Object>) = match &object.content {
                Content::Object(value) => match value.value() {
                    Value::Dictionary(dictionary) => (Some(dictionary), Vec::new()),
                    _ => (None, vec![value]),
                },
                Content::Stream(stream) => (Some(&stream.dictionary), Vec::new()),
            };
            let mut stack: Vec<(&Dictionary, bool)> = Vec::new();
            if let Some(top) = top {
                stack.push((top, object.object().is_some()));
            }
            let mut values = nested;
            loop {
                while let Some(value) = values.pop() {
                    match value.value() {
                        Value::Dictionary(dictionary) => stack.push((dictionary, false)),
                        Value::Array(items) => values.extend(items.iter()),
                        _ => {}
                    }
                }
                let Some((dictionary, is_top)) = stack.pop() else {
                    break;
                };
                if is_action(dictionary) {
                    diagnostics.set_object(Some(*id));
                    let own = is_top.then_some(*id);
                    if let Some(action) = self.read_action(dictionary, own, &mut diagnostics)? {
                        if is_active(&action) {
                            actions.push(ActiveAction {
                                action,
                                holder: *id,
                                page: pages.get(id).copied(),
                            });
                        }
                    }
                    diagnostics.set_object(None);
                }
                values.extend(dictionary.iter().map(|(_, value)| value));
            }
        }
        progress.report(total, total, "objects");
        self.record_diagnostics(diagnostics);
        Ok(ActiveContent {
            actions,
            xfa: self.has_xfa()?,
            attachments: self.attachments()?.len(),
        })
    }

    /// The page of each page object, and of each annotation a page lists.
    fn pages_of_holders(&self) -> Result<HashMap<ObjectId, u32>, PdfError> {
        let mut pages = HashMap::new();
        for number in 1..=self.page_count()? {
            let id = self.page_id(number)?;
            pages.insert(id, number);
            let page = self.required(id)?;
            let Some(dictionary) = page.dictionary() else {
                continue;
            };
            for annot in self
                .get(dictionary, ANNOTS_KEY)?
                .array()
                .unwrap_or_default()
            {
                if let Value::Reference(annot) = annot.value() {
                    pages.entry(*annot).or_insert(number);
                }
            }
        }
        Ok(pages)
    }

    fn has_xfa(&self) -> Result<bool, PdfError> {
        let catalog = self.catalog()?;
        let Some(catalog) = catalog.dictionary() else {
            return Ok(false);
        };
        let form = self.get(catalog, ACRO_FORM_KEY)?;
        Ok(form.dictionary().is_some_and(|form| {
            form.get(XFA_KEY)
                .is_some_and(|xfa| !matches!(xfa.value(), Value::Null))
        }))
    }
}
