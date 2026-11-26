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
use kernkraft_fs::Progress;
use kernkraft_pdf::colour::CmykProfile;
use kernkraft_pdf::doc::{Document, OpenOptions};
use kernkraft_pdf::text::{TextExtractor, TextPage};
use kernkraft_pdf::{EditSession, PdfError};
use serde::Deserialize;
use serde_json::json;

use crate::error::WireError;
use crate::fields::Fields;
use crate::jobs::{JobRequest, Jobs};
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

/// The text pages a session keeps. The page a reader is on, the pages next
/// to it that it reads ahead, and a page read for its lines and then for its
/// blocks are all in reach; the bound keeps a search of a long document from
/// holding every page it passes.
const TEXT_PAGES_KEPT: usize = 8;

/// A [`TextExtractor`] and the text epoch of the snapshots it reads.
struct TextCache {
    epoch: u64,
    extractor: SharedExtractor,
}

/// The text of one page, read at a text epoch.
struct KeptPage {
    epoch: u64,
    page: u32,
    text: Arc<TextPage>,
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
    /// The text epoch of each snapshot: `epochs[i]` is snapshot `i`'s. A
    /// step that leaves the text of every page as it was (an annotation
    /// added, changed or taken away) gives the snapshot it makes the epoch
    /// of the one it leaves; any other step gives it one nobody has had. So
    /// two snapshots of one epoch have the same page contents, fonts and
    /// structure tree, and what was read from one is true of the other.
    epochs: Vec<u64>,
    /// The last epoch given out. Epochs are never reused, so a snapshot
    /// made after an undo does not take the epoch of one that was dropped.
    last_epoch: u64,
    /// The extractor of the snapshots of one epoch. Building one costs the
    /// walk of the structure tree and of the optional-content
    /// configuration, which is far more than reading a page, so it is kept
    /// until a step changes the text. It reads the snapshot it was built
    /// from, which differs from the session's only in annotations.
    text: Option<TextCache>,
    /// The text of the pages read lately, the least recently used first.
    pages: Vec<KeptPage>,
}

impl Session {
    pub fn new(edit: EditSession) -> Self {
        Self {
            edit,
            revision: 0,
            steps: Vec::new(),
            at: 0,
            epochs: vec![0],
            last_epoch: 0,
            text: None,
            pages: Vec::new(),
        }
    }

    /// Records a step the session took: `changed`, and drops the steps that
    /// could have been redone. `keeps_text` says that the step left the text
    /// of every page as it was; a step is taken to change it unless the
    /// caller knows it does not.
    pub fn did(&mut self, changed: Changed, keeps_text: bool) {
        self.steps.truncate(self.at);
        self.epochs.truncate(self.at + 1);
        let epoch = if keeps_text {
            self.text_epoch()
        } else {
            self.last_epoch += 1;
            self.last_epoch
        };
        self.epochs.push(epoch);
        self.steps.push(changed);
        self.at += 1;
        self.revision += 1;
    }

    /// The text epoch of the snapshot the session stands on.
    pub fn text_epoch(&self) -> u64 {
        self.epochs[self.at]
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
        if let Some(extractor) = self.kept_extractor() {
            return Ok(extractor);
        }
        let extractor: SharedExtractor = Arc::new(Mutex::new(TextExtractor::new(self.document())?));
        self.text = Some(TextCache {
            epoch: self.text_epoch(),
            extractor: extractor.clone(),
        });
        Ok(extractor)
    }

    /// The extractor kept for the snapshot the session stands on, if there
    /// is one.
    fn kept_extractor(&self) -> Option<SharedExtractor> {
        let epoch = self.text_epoch();
        self.text
            .as_ref()
            .filter(|cache| cache.epoch == epoch)
            .map(|cache| cache.extractor.clone())
    }

    /// The text of `page` read at `epoch`, if it is kept; it is then the
    /// most recently used.
    fn kept_page(&mut self, epoch: u64, page: u32) -> Option<Arc<TextPage>> {
        let at = self
            .pages
            .iter()
            .position(|kept| kept.epoch == epoch && kept.page == page)?;
        let kept = self.pages.remove(at);
        let text = kept.text.clone();
        self.pages.push(kept);
        Some(text)
    }

    /// Keeps `text`, read at `epoch`, unless a step has since changed the
    /// text, and lets the least recently used page go when there are too
    /// many.
    fn keep_page(&mut self, epoch: u64, text: Arc<TextPage>) {
        if self.text_epoch() != epoch {
            return;
        }
        let page = text.page;
        self.pages
            .retain(|kept| !(kept.epoch == epoch && kept.page == page));
        self.pages.push(KeptPage { epoch, page, text });
        if self.pages.len() > TEXT_PAGES_KEPT {
            self.pages.remove(0);
        }
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
    pub(crate) jobs: Jobs,
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
            jobs: Jobs::default(),
            options,
            fonts: RwLock::new(Arc::new(FontList::default())),
            cmyk: RwLock::new(None),
        }
    }

    /// The limits files are opened under.
    pub fn options(&self) -> &OpenOptions {
        &self.options
    }

    /// Stops the job a long request was given (its `job` field) at the
    /// library's next safe point: the request fails with `Cancelled` and
    /// leaves nothing behind. `running` says whether the job was running;
    /// a cancel for a job that has not begun is kept and applies as it
    /// begins (see [`crate::jobs`]).
    pub fn cancel_job(&self, request: JobRequest) -> Fields {
        self.jobs.cancel(request.job)
    }

    /// How far a job has come: `running`, `done` out of `total` units of
    /// `stage`. A job that is not running has nothing counted.
    pub fn job_progress(&self, request: JobRequest) -> Fields {
        self.jobs.progress(request.job)
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

    /// The document as it stands, its revision, and the extractor for its
    /// text: the one kept if no step has changed the text since it was
    /// built (a step that only adds, changes or takes away an annotation does
    /// not), else a new one. The new one is built outside the session's lock,
    /// so a long build does not hold up another request; if a step changed
    /// the text meanwhile it is used for this request and not kept.
    pub fn text_extractor(
        &self,
        handle: i64,
    ) -> Result<(Document, u64, SharedExtractor), WireError> {
        let (document, revision, epoch, kept) = self.sessions.with(handle, |session| {
            Ok((
                session.document().clone(),
                session.revision,
                session.text_epoch(),
                session.kept_extractor(),
            ))
        })?;
        let extractor = match kept {
            Some(extractor) => extractor,
            None => self.built_extractor(handle, &document, epoch)?,
        };
        Ok((document, revision, extractor))
    }

    /// An extractor for `document`, of text epoch `epoch`, built outside the
    /// session's lock and kept if the session is still at that epoch.
    fn built_extractor(
        &self,
        handle: i64,
        document: &Document,
        epoch: u64,
    ) -> Result<SharedExtractor, WireError> {
        let built: SharedExtractor = Arc::new(Mutex::new(TextExtractor::new(document)?));
        self.sessions.with(handle, |session| {
            if session.text_epoch() != epoch {
                return Ok(built.clone());
            }
            Ok(match &session.text {
                Some(cache) if cache.epoch == epoch => cache.extractor.clone(),
                _ => {
                    session.text = Some(TextCache {
                        epoch,
                        extractor: built.clone(),
                    });
                    built.clone()
                }
            })
        })
    }

    /// Page `page` of the session's text, read through the kept extractor,
    /// or the page as already read if no step has changed the text since.
    /// The document and revision are those the request ran at; a block
    /// index is good only at that revision, though the text is the same at
    /// every revision of one text epoch.
    pub fn text_page(
        &self,
        handle: i64,
        page: u32,
        progress: &dyn Progress,
    ) -> Result<(Document, u64, Arc<TextPage>), WireError> {
        let (document, revision, epoch, kept_extractor, kept_page) =
            self.sessions.with(handle, |session| {
                let epoch = session.text_epoch();
                Ok((
                    session.document().clone(),
                    session.revision,
                    epoch,
                    session.kept_extractor(),
                    session.kept_page(epoch, page),
                ))
            })?;
        if let Some(text) = kept_page {
            return Ok((document, revision, text));
        }
        let extractor = match kept_extractor {
            Some(extractor) => extractor,
            None => self.built_extractor(handle, &document, epoch)?,
        };
        let text = Arc::new(
            extractor
                .lock()
                .map_err(|_| WireError::session_poisoned(handle))?
                .page(page, progress)?,
        );
        self.sessions.with(handle, |session| {
            session.keep_page(epoch, text.clone());
            Ok(())
        })?;
        Ok((document, revision, text))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::path::Path;

    use kernkraft_fs::NoProgress;
    use kernkraft_pdf::file::ReadLimits;

    use super::*;

    const OPTIONS: OpenOptions = OpenOptions {
        limits: ReadLimits {
            max_object_elements: 1 << 20,
            max_xref_entries: 1 << 24,
            max_object_stream_bytes: 1 << 26,
        },
        cached_objects: 64,
        cached_object_streams: 4,
        max_diagnostics: 100,
    };

    fn session() -> Session {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/kern-sample.pdf");
        let document = Document::open(&path, &OPTIONS, "").unwrap();
        Session::new(EditSession::new(document).unwrap())
    }

    /// A page of text numbered `page`, whatever it holds.
    fn text_of(session: &Session, page: u32) -> Arc<TextPage> {
        let mut text = TextExtractor::new(session.document())
            .unwrap()
            .page(1, &NoProgress)
            .unwrap();
        text.page = page;
        Arc::new(text)
    }

    fn kept_numbers(session: &Session) -> Vec<u32> {
        session.pages.iter().map(|kept| kept.page).collect()
    }

    #[test]
    fn a_ninth_page_takes_the_place_of_the_one_used_longest_ago() {
        let mut session = session();
        let epoch = session.text_epoch();
        for page in 1..=TEXT_PAGES_KEPT as u32 {
            session.keep_page(epoch, text_of(&session, page));
        }
        assert_eq!(kept_numbers(&session), (1..=8).collect::<Vec<_>>());

        // Reading page 1 makes it the most recent, so page 2 goes next.
        assert!(session.kept_page(epoch, 1).is_some());
        session.keep_page(epoch, text_of(&session, 9));
        assert_eq!(kept_numbers(&session), vec![3, 4, 5, 6, 7, 8, 1, 9]);
        assert!(session.kept_page(epoch, 2).is_none());
    }

    #[test]
    fn a_page_kept_again_is_kept_once() {
        let mut session = session();
        let epoch = session.text_epoch();
        session.keep_page(epoch, text_of(&session, 1));
        session.keep_page(epoch, text_of(&session, 2));
        session.keep_page(epoch, text_of(&session, 1));
        assert_eq!(kept_numbers(&session), vec![2, 1]);
    }

    #[test]
    fn a_page_read_at_an_epoch_the_session_has_left_is_not_kept() {
        let mut session = session();
        let old = session.text_epoch();
        let text = text_of(&session, 1);
        session.did(Changed::All, false);
        assert_ne!(session.text_epoch(), old);
        session.keep_page(old, text);
        assert!(session.pages.is_empty());
    }

    #[test]
    fn a_text_epoch_is_given_to_a_step_only_when_it_changes_the_text() {
        let mut session = session();
        let first = session.text_epoch();
        session.did(Changed::one(1), true);
        assert_eq!(session.text_epoch(), first);
        session.did(Changed::one(1), false);
        let changed = session.text_epoch();
        assert_ne!(changed, first);
        session.did(Changed::one(1), true);
        assert_eq!(session.text_epoch(), changed);
    }
}
