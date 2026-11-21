// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Actions: ISO 32000-1:2008, 12.6.2 (Table 193), 12.6.4 (Table 198 and
//! the entries of each type that say where an action leads), and 12.7.5
//! for the form actions. Nothing here performs an action: an action is
//! read so that the application can show what it would do.
//!
//! An action's `Next` is one action or an array of them, each with a
//! `Next` of its own; [`Document::actions`] gives the whole sequence in
//! the order 12.6.2 performs it, every action once.

use std::collections::HashSet;

use crate::destination::{Destination, DestinationFile};
use crate::diagnostic::Diagnostics;
use crate::doc::{Document, TextString};
use crate::filespec::FileSpec;
use crate::syntax::{Dictionary, Name, Object, ObjectId, Value};
use crate::PdfError;

const S_KEY: &[u8] = b"S";
const NEXT_KEY: &[u8] = b"Next";
const D_KEY: &[u8] = b"D";
const F_KEY: &[u8] = b"F";
const URI_KEY: &[u8] = b"URI";
const WIN_KEY: &[u8] = b"Win";
const JS_KEY: &[u8] = b"JS";

/// The type of an action (Table 198).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionKind {
    GoTo,
    GoToRemote,
    GoToEmbedded,
    Launch,
    Thread,
    Uri,
    Sound,
    Movie,
    Hide,
    Named,
    SubmitForm,
    ResetForm,
    ImportData,
    JavaScript,
    SetOcgState,
    Rendition,
    Transition,
    GoTo3DView,
    /// A type Table 198 does not define, by its name.
    Other(Name),
}

impl ActionKind {
    fn from_name(name: &Name) -> Self {
        match name.as_bytes() {
            b"GoTo" => ActionKind::GoTo,
            b"GoToR" => ActionKind::GoToRemote,
            b"GoToE" => ActionKind::GoToEmbedded,
            b"Launch" => ActionKind::Launch,
            b"Thread" => ActionKind::Thread,
            b"URI" => ActionKind::Uri,
            b"Sound" => ActionKind::Sound,
            b"Movie" => ActionKind::Movie,
            b"Hide" => ActionKind::Hide,
            b"Named" => ActionKind::Named,
            b"SubmitForm" => ActionKind::SubmitForm,
            b"ResetForm" => ActionKind::ResetForm,
            b"ImportData" => ActionKind::ImportData,
            b"JavaScript" => ActionKind::JavaScript,
            b"SetOCGState" => ActionKind::SetOcgState,
            b"Rendition" => ActionKind::Rendition,
            b"Trans" => ActionKind::Transition,
            b"GoTo3DView" => ActionKind::GoTo3DView,
            _ => ActionKind::Other(name.clone()),
        }
    }
}

/// One action, and where it leads when its type says so.
#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    pub kind: ActionKind,
    /// The action's own object, when it is an indirect one.
    pub object: Option<ObjectId>,
    /// `D` of a go-to action: in this document for `GoTo`, in the other
    /// file for `GoToR` (its page an index counted from 0, 12.6.4.3).
    /// `None` for a named destination of another file, which cannot be
    /// looked up here, and for one that could not be read.
    pub destination: Option<Destination>,
    /// The name `D` gives a destination in another file (12.3.2.3).
    pub destination_name: Option<Vec<u8>>,
    /// `F` of `GoToR`, `GoToE`, `Launch`, `SubmitForm` and `ImportData`
    /// (for `Launch`, `Win`'s `F` when there is no `F`).
    pub file: Option<FileSpec>,
    /// `URI` of a URI action: 7-bit ASCII (12.6.4.7), bytes outside it
    /// read as U+FFFD.
    pub uri: Option<TextString>,
    /// Whether the action carries a JavaScript script: a JavaScript
    /// action (12.6.4.16), or a rendition action with `JS` (12.6.4.13).
    pub script: bool,
}

impl Document {
    /// The action `object` is, followed by every action its `Next`
    /// entries chain, in the order they are performed. An indirect action
    /// or `Next` array reached again is not followed again. A dictionary
    /// that has no `S` name is not an action, and gives none.
    pub fn actions(&self, object: &Object) -> Result<Vec<Action>, PdfError> {
        let mut diagnostics = Diagnostics::new();
        let actions = self.action_sequence(object, &mut diagnostics);
        self.record_diagnostics(diagnostics);
        actions
    }

    pub(crate) fn action_sequence(
        &self,
        object: &Object,
        diagnostics: &mut Diagnostics,
    ) -> Result<Vec<Action>, PdfError> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut pending = vec![object.clone()];
        while let Some(object) = pending.pop() {
            let resolved = self.resolve(&object)?;
            if let Some(id) = resolved.id() {
                if !seen.insert(id) {
                    continue;
                }
            }
            let Some(dictionary) = resolved.dictionary() else {
                continue;
            };
            let Some(action) = self.read_action(dictionary, resolved.id(), diagnostics)? else {
                continue;
            };
            out.push(action);
            if let Some(next) = dictionary.get(NEXT_KEY) {
                let resolved = self.resolve(next)?;
                match resolved.value() {
                    // An array met again would start the same actions
                    // again: an indirect one is followed once.
                    Value::Array(_) if resolved.id().is_some_and(|id| !seen.insert(id)) => {}
                    Value::Array(items) => pending.extend(items.iter().rev().cloned()),
                    Value::Dictionary(_) => pending.push(next.clone()),
                    _ => {}
                }
            }
        }
        Ok(out)
    }

    /// The action `dictionary` is, object `id` when it is an indirect
    /// one. `None` when it has no `S` name.
    pub(crate) fn read_action(
        &self,
        dictionary: &Dictionary,
        id: Option<ObjectId>,
        diagnostics: &mut Diagnostics,
    ) -> Result<Option<Action>, PdfError> {
        let Value::Name(kind) = self.get(dictionary, S_KEY)?.value().clone() else {
            return Ok(None);
        };
        let kind = ActionKind::from_name(&kind);
        let mut action = Action {
            kind,
            object: id,
            destination: None,
            destination_name: None,
            file: None,
            uri: None,
            script: false,
        };
        let offset = self.offset_of(id);
        match action.kind {
            ActionKind::GoTo => {
                if let Some(d) = dictionary.get(D_KEY) {
                    action.destination =
                        self.destination(d, DestinationFile::This, offset, diagnostics)?;
                }
            }
            ActionKind::GoToRemote | ActionKind::GoToEmbedded => {
                action.file = self.file_entry(dictionary)?;
                if let Some(d) = dictionary.get(D_KEY) {
                    match self.resolve(d)?.value() {
                        Value::Name(name) => {
                            action.destination_name = Some(name.as_bytes().to_vec())
                        }
                        Value::String(name) => {
                            action.destination_name = Some(name.as_bytes().to_vec())
                        }
                        _ => {
                            action.destination =
                                self.destination(d, DestinationFile::Other, offset, diagnostics)?
                        }
                    }
                }
            }
            ActionKind::Launch => {
                action.file = match self.file_entry(dictionary)? {
                    Some(file) => Some(file),
                    None => {
                        let win = self.get(dictionary, WIN_KEY)?;
                        match win.dictionary() {
                            Some(win) => self.file_entry(win)?,
                            None => None,
                        }
                    }
                };
            }
            ActionKind::SubmitForm | ActionKind::ImportData => {
                action.file = self.file_entry(dictionary)?;
            }
            ActionKind::JavaScript => action.script = true,
            ActionKind::Rendition => {
                // `JS` may be a stream, which `get` does not resolve.
                action.script = dictionary
                    .get(JS_KEY)
                    .is_some_and(|js| !matches!(js.value(), Value::Null));
            }
            ActionKind::Uri => {
                if let Value::String(uri) = self.get(dictionary, URI_KEY)?.value() {
                    action.uri = Some(ascii(uri.as_bytes()));
                }
            }
            _ => {}
        }
        Ok(Some(action))
    }

    fn file_entry(&self, dictionary: &Dictionary) -> Result<Option<FileSpec>, PdfError> {
        match dictionary.get(F_KEY) {
            Some(file) => self.file_spec(file),
            None => Ok(None),
        }
    }
}

/// 7-bit ASCII bytes as text; any other byte is U+FFFD and makes the text
/// inexact.
fn ascii(bytes: &[u8]) -> TextString {
    let exact = bytes.is_ascii();
    let text = bytes
        .iter()
        .map(|&byte| {
            if byte.is_ascii() {
                char::from(byte)
            } else {
                char::REPLACEMENT_CHARACTER
            }
        })
        .collect();
    TextString {
        text,
        language: None,
        exact,
    }
}
