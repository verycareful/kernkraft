// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The sessions and the state they share.
//!
//! Each open document is an [`EditSession`] with a revision number and a
//! record of which pages each of its steps changed. The record is the
//! adapter's, because `kernkraft-pdf` keeps snapshots and not what differs
//! between them: after an edit, an undo or a redo Kotlin is told which pages
//! to draw again.

use std::sync::{Arc, Mutex, RwLock};

use kernkraft_font::FontList;
use kernkraft_fs::NoProgress;
use kernkraft_pdf::colour::CmykProfile;
use kernkraft_pdf::doc::{Document, OpenOptions};
use kernkraft_pdf::text::{TextExtractor, TextPage};
use kernkraft_pdf::{EditSession, PdfError};
use serde::Deserialize;
use serde_json::json;

use crate::error::WireError;
use crate::fields::Fields;
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

/// The reader of a snapshot's text, shared by the requests that read it. It
/// is behind a lock of its own because a read takes `&mut` (the interpreter
/// keeps the fonts it has read), and the session's lock is not held while a
/// page is read.
pub type SharedExtractor = Arc<Mutex<TextExtractor>>;

/// A [`TextExtractor`] and the revision of the snapshot it reads.
struct TextCache {
    revision: u64,
    extractor: SharedExtractor,
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
    /// The extractor of the snapshot at `revision`. Building one costs the
    /// walk of the structure tree and of the optional-content
    /// configuration, which is far more than reading a page, so it is kept
    /// until the revision moves.
    text: Option<TextCache>,
}

impl Session {
    pub fn new(edit: EditSession) -> Self {
        Self {
            edit,
            revision: 0,
            steps: Vec::new(),
            at: 0,
            text: None,
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

    /// The extractor of the snapshot the session stands on, built here when
    /// there is none for this revision. For a step that already holds the
    /// session, such as a text edit; a read builds it outside the lock
    /// ([`Engine::text_extractor`]).
    pub fn extractor(&mut self) -> Result<SharedExtractor, PdfError> {
        if let Some(cache) = &self.text {
            if cache.revision == self.revision {
                return Ok(cache.extractor.clone());
            }
        }
        let extractor: SharedExtractor = Arc::new(Mutex::new(TextExtractor::new(self.document())?));
        self.text = Some(TextCache {
            revision: self.revision,
            extractor: extractor.clone(),
        });
        Ok(extractor)
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

/// A request that names a session.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandleRequest {
    pub handle: i64,
}

/// A request that names a session and one of its pages, counted from 1.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageRequest {
    pub handle: i64,
    pub page: u32,
}

/// Every open document, the application's fonts and CMYK profile, and the
/// limits a file is opened under.
pub struct Engine {
    pub(crate) sessions: Registry<Session>,
    pub(crate) options: OpenOptions,
    fonts: RwLock<Arc<FontList>>,
    cmyk: RwLock<Option<CmykProfile>>,
}

impl Engine {
    /// An engine with no documents open and no fonts. Every file it opens is
    /// opened under `options`: what a document may cost depends on the
    /// device, so the adapter sets it.
    pub fn new(options: OpenOptions) -> Self {
        Self {
            sessions: Registry::new(),
            options,
            fonts: RwLock::new(Arc::new(FontList::default())),
            cmyk: RwLock::new(None),
        }
    }

    /// The limits files are opened under.
    pub fn options(&self) -> &OpenOptions {
        &self.options
    }

    /// The fonts set by `set_system_fonts`; none until then.
    pub fn fonts(&self) -> Arc<FontList> {
        self.fonts
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The CMYK profile set by `set_cmyk_profile`; none until then, and the
    /// bundled one is used.
    pub fn cmyk_profile(&self) -> Option<CmykProfile> {
        self.cmyk
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn set_cmyk(&self, profile: Option<CmykProfile>) {
        *self
            .cmyk
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = profile;
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

    /// The document as it stands, its revision, and the extractor of that
    /// snapshot: the one the last read made if the revision has not moved
    /// since, else a new one. The new one is built outside the session's
    /// lock, so a long build does not hold up another request; if the
    /// revision moved meanwhile it is used for this request and not kept.
    pub fn text_extractor(
        &self,
        handle: i64,
    ) -> Result<(Document, u64, SharedExtractor), WireError> {
        let (document, revision, kept) = self.sessions.with(handle, |session| {
            let kept = session
                .text
                .as_ref()
                .filter(|cache| cache.revision == session.revision)
                .map(|cache| cache.extractor.clone());
            Ok((session.document().clone(), session.revision, kept))
        })?;
        if let Some(extractor) = kept {
            return Ok((document, revision, extractor));
        }
        let built: SharedExtractor = Arc::new(Mutex::new(TextExtractor::new(&document)?));
        let extractor = self.sessions.with(handle, |session| {
            if session.revision != revision {
                return Ok(built.clone());
            }
            Ok(match &session.text {
                Some(cache) if cache.revision == revision => cache.extractor.clone(),
                _ => {
                    session.text = Some(TextCache {
                        revision,
                        extractor: built.clone(),
                    });
                    built.clone()
                }
            })
        })?;
        Ok((document, revision, extractor))
    }

    /// Page `page` of the session's text, read through the kept extractor.
    /// The document and revision are the ones the text was read at.
    pub fn text_page(
        &self,
        handle: i64,
        page: u32,
    ) -> Result<(Document, u64, TextPage), WireError> {
        let (document, revision, extractor) = self.text_extractor(handle)?;
        let text = extractor
            .lock()
            .map_err(|_| WireError::session_poisoned(handle))?
            .page(page, &NoProgress)?;
        Ok((document, revision, text))
    }
}
