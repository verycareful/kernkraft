// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! PDF documents for the viewer and editor: `kernkraft-pdf-engine`'s
//! operations as methods of [`Kernkraft`].
//!
//! A document is opened into a session the plugin holds, named by the
//! `handle` the open returns; the webview passes it to every later call and
//! closes it when done. A session belongs to the window that opened it and
//! goes with the window. Every answer is the engine's: a JSON object whose
//! fields are described on the engine's operations, and geometry is
//! fractions of the displayed page (see `kernkraft_pdf_engine::wire`).
//!
//! A command that can take long (`pdf_text`, `pdf_search`, `pdf_apply`,
//! `pdf_save`, `pdf_compact`, `pdf_render`, `pdf_merge`, `pdf_extract`,
//! `pdf_split`, `pdf_set_system_fonts`) takes an optional `job`, a number the
//! webview chooses and never uses twice. [`Kernkraft::pdf_cancel_job`] stops
//! the command with that number (it fails with `Cancelled` and leaves
//! nothing behind) and [`Kernkraft::pdf_job_progress`] says how far it has
//! come. Closing a document stops the jobs working on it; closing a window
//! stops the merges, extracts and splits it started.
//!
//! The file is the webview's to save. [`Kernkraft::pdf_open`] answers with a
//! [`WireStamp`] of the file as opened, and [`Kernkraft::pdf_save`] writes
//! the opened bytes and one update to a path, as far as its [`WireExpect`]
//! allows (the file with that stamp, nothing, or anything), and answers
//! with the stamp of the file written, as the text commands do. A change
//! to the file meanwhile refuses with `ExternalModification` and nothing is
//! written. Dates in an edit and in a save are PDF dates
//! (`D:YYYYMMDDHHmmSSOHH'mm'`) from the webview's clock: the plugin reads
//! none.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use kernkraft_fs::{BaseDirs, FileStamp, FsError};
use kernkraft_pdf::doc::OpenOptions;
use kernkraft_pdf::file::ReadLimits;
use kernkraft_pdf_engine::colour::CmykProfileRequest;
use kernkraft_pdf_engine::edit::{ApplyRequest, Op};
use kernkraft_pdf_engine::fonts::FontsRequest;
use kernkraft_pdf_engine::jobs::{JobId, JobRequest};
use kernkraft_pdf_engine::open::Source;
use kernkraft_pdf_engine::read::{SearchRequest, TextRequest};
use kernkraft_pdf_engine::render::RenderRequest;
use kernkraft_pdf_engine::stateless::{self, ExtractRequest, MergeRequest, SplitRequest};
use kernkraft_pdf_engine::{Engine, Fields, HandleRequest, PageRequest};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::Runtime;

use crate::error::{PluginError, WireError};
use crate::state::{lock, Kernkraft};
use crate::text_cmds::{WireExpect, WireStamp};
use crate::wire::WirePath;

/// Array items and dictionary entries in one object. A desktop holds a far
/// larger object than a phone does beside the page it draws.
const MAX_OBJECT_ELEMENTS: usize = 4 << 20;
/// Cross-reference entries over all sections of a file: 64 million objects.
const MAX_XREF_ENTRIES: usize = 64 << 20;
/// The decoded bytes of one object stream.
const MAX_OBJECT_STREAM_BYTES: usize = 64 << 20;
/// Objects kept in memory once read; others are read again when used.
const CACHED_OBJECTS: usize = 2048;
/// Decoded object streams kept in memory.
const CACHED_OBJECT_STREAMS: usize = 8;
/// Diagnostics kept; past it they are counted.
const MAX_DIAGNOSTICS: usize = 10_000;

/// What opening a document may cost on a desktop; a file past a limit is
/// refused with `LimitExceeded`.
pub(crate) const OPEN_OPTIONS: OpenOptions = OpenOptions {
    limits: ReadLimits {
        max_object_elements: MAX_OBJECT_ELEMENTS,
        max_xref_entries: MAX_XREF_ENTRIES,
        max_object_stream_bytes: MAX_OBJECT_STREAM_BYTES,
    },
    cached_objects: CACHED_OBJECTS,
    cached_object_streams: CACHED_OBJECT_STREAMS,
    max_diagnostics: MAX_DIAGNOSTICS,
};

/// How deep the font folders are searched. Folders that are links are not
/// entered, so this only bounds a very deep tree.
const MAX_FONT_DEPTH: usize = 16;

/// The font files a folder holds, by extension.
const FONT_EXTENSIONS: [&str; 6] = ["ttf", "otf", "ttc", "otc", "pfb", "pfa"];

/// The open documents, and the window each belongs to.
pub(crate) struct PdfState {
    engine: Engine,
    /// Handle to window label, for the documents opened from a window.
    owners: Mutex<HashMap<i64, String>>,
    /// Job number to window label, for the jobs of a window that work on no
    /// document (merge, extract, split); those that work on one stop when it
    /// is closed.
    job_owners: Mutex<HashMap<JobId, String>>,
}

impl Default for PdfState {
    fn default() -> Self {
        Self {
            engine: Engine::new(OPEN_OPTIONS),
            owners: Mutex::new(HashMap::new()),
            job_owners: Mutex::new(HashMap::new()),
        }
    }
}

/// A PDF to read: its path and the password that opens it, when it has one.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WirePdfSource {
    pub path: WirePath,
    #[serde(default)]
    pub password: Option<String>,
}

impl WirePdfSource {
    fn into_source(self) -> Result<Source, WireError> {
        Ok(Source {
            path: PathBuf::try_from(self.path)?,
            password: self.password.unwrap_or_default(),
        })
    }
}

/// A job of a window that works on no document, listed for as long as it
/// runs.
struct OwnedJob<'a> {
    owners: &'a Mutex<HashMap<JobId, String>>,
    job: Option<JobId>,
}

impl Drop for OwnedJob<'_> {
    fn drop(&mut self) {
        if let Some(job) = self.job {
            lock(self.owners).remove(&job);
        }
    }
}

impl<R: Runtime> Kernkraft<R> {
    fn engine(&self) -> &Engine {
        &self.0.pdf.engine
    }

    /// Lists `job` as belonging to the window `origin` until the guard is
    /// dropped, so that closing the window stops it.
    fn own_job(&self, job: Option<JobId>, origin: Option<String>) -> OwnedJob<'_> {
        if let (Some(job), Some(label)) = (job, origin) {
            lock(&self.0.pdf.job_owners).insert(job, label);
        }
        OwnedJob {
            owners: &self.0.pdf.job_owners,
            job,
        }
    }

    /// Stops the job `job`, which a long command was given. `running` says
    /// whether it was running; a cancel for a job about to begin is kept and
    /// applies as it begins. A document's job also stops when the document
    /// is closed, and a window's other jobs when the window closes.
    pub fn pdf_cancel_job(&self, job: JobId) -> Result<Fields, WireError> {
        Ok(self.engine().cancel_job(JobRequest { job }))
    }

    /// How far a job has come: `running`, `done` out of `total` units of
    /// `stage` (objects, pages or files, as the operation counts them;
    /// `total` is 0 until it knows). The webview asks again for as long as
    /// it wants to show it.
    pub fn pdf_job_progress(&self, job: JobId) -> Result<Fields, WireError> {
        Ok(self.engine().job_progress(JobRequest { job }))
    }

    /// Opens the PDF at `path`. The answer has `handle`, `pages`, `revision`
    /// and the `stamp` of the file as it is now. The stamp is taken before
    /// the file is read, so a file that changes in between is refused at
    /// the save rather than overwritten. `origin` is the window the
    /// document belongs to: it is closed with the window.
    pub fn pdf_open(
        &self,
        source: WirePdfSource,
        origin: Option<String>,
    ) -> Result<Fields, WireError> {
        let source = source.into_source()?;
        let stamp = stamp_of(&source.path)?;
        let mut fields = self.engine().open(source)?;
        if let (Some(label), Some(handle)) = (origin, fields.get("handle").and_then(|h| h.as_i64()))
        {
            lock(&self.0.pdf.owners).insert(handle, label);
        }
        fields.insert("stamp".to_owned(), json!(WireStamp::new(&stamp)));
        Ok(fields)
    }

    /// Closes the document; its handle is never used again.
    pub fn pdf_close(&self, handle: i64) -> Result<Fields, WireError> {
        let closed = self.engine().close(HandleRequest { handle })?;
        lock(&self.0.pdf.owners).remove(&handle);
        Ok(closed)
    }

    /// The displayed size of a page and the box of user space it shows.
    pub fn pdf_page_info(&self, handle: i64, page: u32) -> Result<Fields, WireError> {
        Ok(self.engine().page_info(PageRequest { handle, page })?)
    }

    /// A page's text in reading order: lines, blocks (editable or not, and
    /// why not) and the `revision` a block index is good at.
    pub fn pdf_text(
        &self,
        handle: i64,
        page: u32,
        job: Option<JobId>,
    ) -> Result<Fields, WireError> {
        Ok(self.engine().text(TextRequest { handle, page, job })?)
    }

    /// The matches of `query` on one page.
    pub fn pdf_search(
        &self,
        handle: i64,
        page: u32,
        query: String,
        case_sensitive: bool,
        whole_word: bool,
        job: Option<JobId>,
    ) -> Result<Fields, WireError> {
        Ok(self.engine().search(SearchRequest {
            handle,
            page,
            query,
            case_sensitive,
            whole_word,
            job,
        })?)
    }

    /// A page's links.
    pub fn pdf_links(&self, handle: i64, page: u32) -> Result<Fields, WireError> {
        Ok(self.engine().links(PageRequest { handle, page })?)
    }

    /// A page's annotations.
    pub fn pdf_annotations(&self, handle: i64, page: u32) -> Result<Fields, WireError> {
        Ok(self.engine().annotations(PageRequest { handle, page })?)
    }

    /// Makes one change to the document and answers with the new
    /// `revision`, the `pages` and `changed_pages`, and whether it can be
    /// undone or redone and is `modified`.
    pub fn pdf_apply(&self, handle: i64, op: Op, job: Option<JobId>) -> Result<Fields, WireError> {
        Ok(self.engine().apply(ApplyRequest { handle, op, job })?)
    }

    /// Goes back one change; the answer is that of `pdf_apply`.
    pub fn pdf_undo(&self, handle: i64) -> Result<Fields, WireError> {
        Ok(self.engine().undo(HandleRequest { handle })?)
    }

    /// Goes forward one change; the answer is that of `pdf_apply`.
    pub fn pdf_redo(&self, handle: i64) -> Result<Fields, WireError> {
        Ok(self.engine().redo(HandleRequest { handle })?)
    }

    /// Writes the opened bytes and one update with every change to `path`,
    /// atomically, as far as `expect` allows, and gives the stamp of the
    /// file written. `modified` is the time of the save as a PDF date.
    pub fn pdf_save(
        &self,
        handle: i64,
        path: WirePath,
        expect: WireExpect,
        modified: String,
        job: Option<JobId>,
    ) -> Result<WireStamp, WireError> {
        let path = PathBuf::try_from(path)?;
        let stamp = expect
            .with(|expect| Ok(self.engine().save(handle, &path, expect, &modified, job)?))?;
        Ok(WireStamp::new(&stamp))
    }

    /// The document as it stands, written as a new compacted file `out`
    /// that must not exist; the opened file is untouched. For a file whose
    /// cross-reference had to be rebuilt, which is not saved in place.
    pub fn pdf_compact(
        &self,
        handle: i64,
        out: WirePath,
        job: Option<JobId>,
    ) -> Result<WirePath, WireError> {
        let out = PathBuf::try_from(out)?;
        let written = self
            .engine()
            .compact(kernkraft_pdf_engine::save::CompactRequest { handle, out, job })?;
        Ok(WirePath::from(&written))
    }

    /// Indexes the font files of the user's and the system's font folders
    /// (`$XDG_DATA_HOME/fonts`, `~/.fonts`, and `fonts` under each of
    /// `$XDG_DATA_DIRS`) for every text edit and render after it. The answer
    /// has `faces`, the files `skipped` (not fonts, or fonts that cannot be
    /// indexed) and the `unreadable_dirs` that could not be searched.
    pub fn pdf_set_system_fonts(&self, job: Option<JobId>) -> Result<Fields, WireError> {
        let found = font_files(self.base()?);
        let mut fields = self.engine().set_system_fonts(FontsRequest {
            paths: found.files,
            job,
        })?;
        fields.insert("unreadable_dirs".to_owned(), json!(found.unreadable));
        Ok(fields)
    }

    /// Reads the ICC profile at `path`, a profile for a CMYK colour space,
    /// and uses it for DeviceCMYK in every document opened after, where the
    /// file has no output intent of its own; with no path the bundled
    /// profile is used again. A document already open keeps the profile it
    /// was opened with. Call it once at start. The answer has `source`:
    /// `application`, or `bundled`.
    pub fn pdf_set_cmyk_profile(&self, path: Option<PathBuf>) -> Result<Fields, WireError> {
        Ok(self
            .engine()
            .set_cmyk_profile(CmykProfileRequest { path })?)
    }

    /// The size of the picture `pdf_render` makes: `width`, `height` and
    /// `bytes`. `region` is the part of the displayed page to draw,
    /// `[left, top, right, bottom]` as fractions of it; the whole page when
    /// it is `None`. The width is `width_px`, the height follows.
    pub fn pdf_render_size(
        &self,
        handle: i64,
        page: u32,
        region: Option<[f64; 4]>,
        width_px: u32,
    ) -> Result<Fields, WireError> {
        Ok(self.engine().render_size(RenderRequest {
            handle,
            page,
            region,
            width_px,
            job: None,
        })?)
    }

    /// Draws the page: straight RGBA8, row by row from the top, every alpha
    /// 255, exactly the `bytes` that `pdf_render_size` says.
    pub fn pdf_render(
        &self,
        handle: i64,
        page: u32,
        region: Option<[f64; 4]>,
        width_px: u32,
        job: Option<JobId>,
    ) -> Result<Vec<u8>, WireError> {
        let request = |job| RenderRequest {
            handle,
            page,
            region,
            width_px,
            job,
        };
        let size = self.engine().render_size(request(None))?;
        let bytes = size
            .get("bytes")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| {
                WireError::from(PluginError::Internal(
                    "the render size has no byte count".to_owned(),
                ))
            })?;
        let length = usize::try_from(bytes).map_err(|_| {
            WireError::from(PluginError::InvalidArgument {
                what: "picture size",
                value: bytes.to_string(),
            })
        })?;
        let mut picture = vec![0_u8; length];
        self.engine().render(request(job), &mut picture)?;
        Ok(picture)
    }

    /// Concatenates the sources, in order, into the new file `out`, which
    /// must not exist.
    pub fn pdf_merge(
        &self,
        sources: Vec<WirePdfSource>,
        out: WirePath,
        job: Option<JobId>,
        origin: Option<String>,
    ) -> Result<WirePath, WireError> {
        let request = MergeRequest {
            sources: sources
                .into_iter()
                .map(WirePdfSource::into_source)
                .collect::<Result<_, _>>()?,
            out: PathBuf::try_from(out)?,
            job,
        };
        let _owned = self.own_job(job, origin);
        one_path(stateless::merge_files(self.engine(), request)?)
    }

    /// The listed pages of the source (counted from 1, in the order the new
    /// file shows them) as the new file `out`, which must not exist.
    pub fn pdf_extract(
        &self,
        source: WirePdfSource,
        pages: Vec<u32>,
        out: WirePath,
        job: Option<JobId>,
        origin: Option<String>,
    ) -> Result<WirePath, WireError> {
        let request = ExtractRequest {
            source: source.into_source()?,
            pages,
            out: PathBuf::try_from(out)?,
            job,
        };
        let _owned = self.own_job(job, origin);
        one_path(stateless::extract_pages(self.engine(), request)?)
    }

    /// One new file per range (`1-3,5,7-9`) in the folder `out_dir`, named
    /// after the source, in the order of the ranges. A file that exists is
    /// not replaced, and a split that fails part way removes what it made.
    pub fn pdf_split(
        &self,
        source: WirePdfSource,
        ranges: String,
        out_dir: WirePath,
        job: Option<JobId>,
        origin: Option<String>,
    ) -> Result<Vec<WirePath>, WireError> {
        let request = SplitRequest {
            source: source.into_source()?,
            ranges,
            out_dir: PathBuf::try_from(out_dir)?,
            job,
        };
        let _owned = self.own_job(job, origin);
        let made = stateless::split_by_ranges(self.engine(), request)?;
        Ok(made.iter().map(WirePath::from).collect())
    }

    /// Closes the documents opened from window `label`, or every document
    /// opened from a window for `None`.
    pub(crate) fn pdf_shutdown(&self, label: Option<&str>) {
        let jobs: Vec<JobId> = lock(&self.0.pdf.job_owners)
            .iter()
            .filter(|(_, owner)| label.is_none() || Some(owner.as_str()) == label)
            .map(|(job, _)| *job)
            .collect();
        for job in jobs {
            self.engine().cancel_job(JobRequest { job });
        }
        let handles: Vec<i64> = {
            let mut owners = lock(&self.0.pdf.owners);
            let handles: Vec<i64> = owners
                .iter()
                .filter(|(_, owner)| label.is_none() || Some(owner.as_str()) == label)
                .map(|(handle, _)| *handle)
                .collect();
            for handle in &handles {
                owners.remove(handle);
            }
            handles
        };
        for handle in handles {
            // A handle the webview closed already is not an error here.
            let _ = self.engine().close(HandleRequest { handle });
        }
    }
}

/// The stamp of the file at `path`. A file that is not there is `NotFound`,
/// as opening it reports it.
fn stamp_of(path: &Path) -> Result<FileStamp, FsError> {
    match FileStamp::of(path) {
        Err(FsError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            Err(FsError::NotFound {
                path: path.to_path_buf(),
            })
        }
        other => other,
    }
}

/// The one path of a merge or an extract.
fn one_path(mut made: Vec<PathBuf>) -> Result<WirePath, WireError> {
    match (made.pop(), made.is_empty()) {
        (Some(path), true) => Ok(WirePath::from(&path)),
        _ => Err(WireError::from(PluginError::Internal(
            "the operation did not make exactly one file".to_owned(),
        ))),
    }
}

/// The font files found, and how many folders could not be searched.
struct FontFiles {
    files: Vec<PathBuf>,
    unreadable: usize,
}

/// The font files in the font folders of `base`. A folder that does not
/// exist is not an error; one that exists and cannot be read is counted.
fn font_files(base: &BaseDirs) -> FontFiles {
    let mut roots: Vec<PathBuf> = vec![base.home.join(".fonts"), base.data.join("fonts")];
    roots.extend(base.data_dirs.iter().map(|dir| dir.join("fonts")));
    let mut found = FontFiles {
        files: Vec::new(),
        unreadable: 0,
    };
    for root in roots {
        collect_fonts(&root, 0, &mut found);
    }
    found.files.sort();
    found.files.dedup();
    found
}

fn collect_fonts(dir: &Path, depth: usize, found: &mut FontFiles) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(_) => {
            found.unreadable += 1;
            return;
        }
    };
    for entry in entries {
        let Ok(entry) = entry else {
            found.unreadable += 1;
            continue;
        };
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            found.unreadable += 1;
            continue;
        };
        if kind.is_dir() {
            if depth < MAX_FONT_DEPTH {
                collect_fonts(&path, depth + 1, found);
            }
        } else if is_font_file(&path) {
            found.files.push(path);
        }
    }
}

fn is_font_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            FONT_EXTENSIONS
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::path::Path;

    use tauri::test::{mock_app, MockRuntime};

    use super::*;

    fn kk() -> (tauri::App<MockRuntime>, Kernkraft<MockRuntime>) {
        let app = mock_app();
        let kk = Kernkraft::new(app.handle().clone(), crate::Config::default());
        (app, kk)
    }

    fn open_from(kk: &Kernkraft<MockRuntime>, window: &str) -> i64 {
        let source = WirePdfSource {
            path: WirePath::from(
                &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/kern-sample.pdf"),
            ),
            password: None,
        };
        kk.pdf_open(source, Some(window.to_owned())).unwrap()["handle"]
            .as_i64()
            .unwrap()
    }

    fn is_open(kk: &Kernkraft<MockRuntime>, handle: i64) -> bool {
        kk.pdf_page_info(handle, 1).is_ok()
    }

    #[test]
    fn closing_a_window_closes_its_documents_and_no_others() {
        let (_app, kk) = kk();
        let first = open_from(&kk, "one");
        let second = open_from(&kk, "two");
        let again = open_from(&kk, "one");

        kk.pdf_shutdown(Some("one"));

        assert!(!is_open(&kk, first));
        assert!(!is_open(&kk, again));
        assert!(is_open(&kk, second));
        kk.pdf_shutdown(None);
        assert!(!is_open(&kk, second));
    }

    #[test]
    fn a_document_closed_by_the_webview_is_not_closed_again_with_its_window() {
        let (_app, kk) = kk();
        let handle = open_from(&kk, "one");
        kk.pdf_close(handle).unwrap();

        kk.pdf_shutdown(Some("one"));

        assert!(lock(&kk.0.pdf.owners).is_empty());
    }

    fn merge(kk: &Kernkraft<MockRuntime>, dir: &Path, job: u64) -> Result<WirePath, WireError> {
        let sample = || WirePdfSource {
            path: WirePath::from(
                &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/kern-sample.pdf"),
            ),
            password: None,
        };
        kk.pdf_merge(
            vec![sample(), sample()],
            WirePath::from(&dir.join(format!("merged-{job}.pdf"))),
            Some(job),
            None,
        )
    }

    #[test]
    fn closing_a_window_stops_the_jobs_it_started_that_work_on_no_document() {
        let (_app, kk) = kk();
        let dir = tempfile::tempdir().unwrap();
        let _owned = kk.own_job(Some(5), Some("one".to_owned()));

        kk.pdf_shutdown(Some("one"));

        // The cancel is for job 5, which has not begun in this test: it is
        // kept, and the merge that takes the number is stopped.
        assert_eq!(merge(&kk, dir.path(), 5).unwrap_err().kind, "Cancelled");
        assert!(!dir.path().join("merged-5.pdf").exists());
    }

    #[test]
    fn closing_another_window_leaves_the_job_alone() {
        let (_app, kk) = kk();
        let dir = tempfile::tempdir().unwrap();
        let _owned = kk.own_job(Some(6), Some("one".to_owned()));

        kk.pdf_shutdown(Some("two"));

        assert!(merge(&kk, dir.path(), 6).is_ok());
    }

    #[test]
    fn a_job_is_no_window_s_once_it_is_over() {
        let (_app, kk) = kk();
        let owned = kk.own_job(Some(7), Some("one".to_owned()));
        assert!(lock(&kk.0.pdf.job_owners).contains_key(&7));

        drop(owned);

        assert!(lock(&kk.0.pdf.job_owners).is_empty());
    }
}
