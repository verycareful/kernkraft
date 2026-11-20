// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The sessions and the state they share.
//!
//! Each open document is an [`EditSession`] with a revision number and a
//! record of which pages each of its steps changed. The record is the
//! adapter's, because `kernkraft-pdf` keeps snapshots and not what differs
//! between them: after an edit, an undo or a redo Kotlin is told which pages
//! to draw again.

use std::sync::{Arc, OnceLock, RwLock};

use kernkraft_font::FontList;
use kernkraft_pdf::doc::Document;
use kernkraft_pdf::{EditSession, PdfError};
use serde_json::json;

use crate::error::WireError;
use crate::guard::Fields;
use crate::registry::Registry;

/// The pages a step changed, in the document the step leaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Changed {
    /// These pages, counted from 1.
    Pages(Vec<u32>),
    /// Every page: the step changed the page tree, so numbers moved.
    All,
}

impl Changed {
    pub fn one(page: u32) -> Self {
        Self::Pages(vec![page])
    }

    /// The page numbers, for a document of `count` pages. A page past the
    /// end (the step before an undone page removal) is left out.
    pub fn pages(&self, count: u32) -> Vec<u32> {
        match self {
            Self::All => (1..=count).collect(),
            Self::Pages(pages) => pages.iter().copied().filter(|p| *p <= count).collect(),
        }
    }
}

/// One open document.
pub struct Session {
    pub edit: EditSession,
    /// 0 when opened; one more for every step done, undone or redone. A
    /// text block index is good only at the revision it was read at.
    pub revision: u64,
    /// What each snapshot step changed: `steps[i]` takes snapshot `i` to
    /// `i + 1`.
    steps: Vec<Changed>,
    /// The snapshot the session stands on.
    at: usize,
}

impl Session {
    pub fn new(edit: EditSession) -> Self {
        Self {
            edit,
            revision: 0,
            steps: Vec::new(),
            at: 0,
        }
    }

    /// Records a step the session took: `changed`, and drops the steps that
    /// could have been redone.
    pub fn did(&mut self, changed: Changed) {
        self.steps.truncate(self.at);
        self.steps.push(changed);
        self.at += 1;
        self.revision += 1;
    }

    /// Goes back one step. What it changed, `None` when there is none.
    pub fn undid(&mut self) -> Option<Changed> {
        if !self.edit.undo() {
            return None;
        }
        self.at -= 1;
        self.revision += 1;
        self.steps.get(self.at).cloned()
    }

    /// Goes forward one step. What it changed, `None` when there is none.
    pub fn redid(&mut self) -> Option<Changed> {
        if !self.edit.redo() {
            return None;
        }
        let changed = self.steps.get(self.at).cloned();
        self.at += 1;
        self.revision += 1;
        changed
    }

    /// What the step just taken changed.
    pub fn last_changed(&self) -> Changed {
        self.steps
            .get(self.at - 1)
            .cloned()
            .unwrap_or(Changed::Pages(Vec::new()))
    }

    pub fn document(&self) -> &Document {
        self.edit.document()
    }

    /// The fields every step answers with.
    pub fn answer(&self, changed: &Changed) -> Result<Fields, PdfError> {
        let pages = self.document().page_count()?;
        let mut fields = Fields::new();
        fields.insert("revision".to_owned(), json!(self.revision));
        fields.insert("pages".to_owned(), json!(pages));
        fields.insert("changed_pages".to_owned(), json!(changed.pages(pages)));
        fields.insert("can_undo".to_owned(), json!(self.edit.can_undo()));
        fields.insert("can_redo".to_owned(), json!(self.edit.can_redo()));
        fields.insert("modified".to_owned(), json!(self.edit.is_modified()));
        Ok(fields)
    }
}

/// Every open document and the application's fonts.
pub struct Engine {
    pub(crate) sessions: Registry<Session>,
    fonts: RwLock<Arc<FontList>>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        Self {
            sessions: Registry::new(),
            fonts: RwLock::new(Arc::new(FontList::default())),
        }
    }

    /// The fonts set by `setSystemFonts`; none until then.
    pub fn fonts(&self) -> Arc<FontList> {
        self.fonts
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn set_fonts(&self, list: FontList) {
        *self
            .fonts
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(list);
    }

    /// The document as it stands and its revision, taken so that a long
    /// read or render does not hold the session's lock.
    pub fn snapshot(&self, handle: i64) -> Result<(Document, u64), WireError> {
        self.sessions.with(handle, |session| {
            Ok((session.document().clone(), session.revision))
        })
    }
}

/// The engine the JNI exports use.
pub fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(Engine::new)
}
