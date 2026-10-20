// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Name search and the Documents view: a walk of folders that streams what
//! it finds.
//!
//! Both are background operations ([`OpId`], `op-progress`, `op-done`,
//! [`Kernkraft::cancel_op`]; see the `ops` module) over
//! `kernkraft_fs::scan`, which follows symlinks to folders and enters each
//! folder once by device and inode, so a symlink loop ends. Matches go out
//! as [`EVENT_SEARCH_HIT`] in batches of [`SEARCH_BATCH`], and the last,
//! shorter batch before `op-done`, whose `scan` holds the count. Entries are
//! sent as found and not kept, so a walk of a large tree holds one batch.
//!
//! A search or Documents scan only reads, so unlike the file operations it
//! does not make undo or redo wait, and may run while one does.

use std::path::PathBuf;
use std::sync::Mutex;

use kernkraft_fs::{scan, Entry, EntryKind, Progress, ScanOptions};
use kernkraft_xdg_apps::MimeDb;
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Runtime};

use crate::error::{PluginError, WireError};
use crate::fs_cmds::WireEntry;
use crate::ops::{Job, OpId};
use crate::state::{lock, Kernkraft};
use crate::wire::WirePath;

pub const EVENT_SEARCH_HIT: &str = "kernkraft://search-hit";

/// Entries per [`EVENT_SEARCH_HIT`]: the view renders its list once per
/// batch, so a search with thousands of hits re-renders tens of times, not
/// thousands, while the first hits still show at once.
pub const SEARCH_BATCH: usize = 200;

/// Payload of [`EVENT_SEARCH_HIT`]: found entries of operation `op`,
/// started under `key`, in walk order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHitEvent {
    pub key: String,
    pub op: OpId,
    pub entries: Vec<WireEntry>,
}

/// What a finished walk reports in `op-done`'s `scan`: how many entries
/// matched, the folders (or roots) that could not be read, and whether it
/// was cancelled.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireScanDone {
    pub found: u64,
    pub unreadable: Vec<WirePath>,
    pub cancelled: bool,
}

/// Which entries a walk keeps.
pub(crate) enum ScanFilter {
    /// Names containing this text, compared in lower case.
    Name(String),
    /// Text files, by name.
    Documents,
}

/// Whether `e` is a document for the Documents view (design D5.1: in
/// 0.1.0.0 a supported document is a text file): a regular file, or a
/// symlink to one, whose name types it as text (`MimeDb::is_text`, which
/// takes subclasses of `text/plain` too). Typed by name only, never by
/// content: a scan of a home folder must not open every file in it.
fn is_document(e: &Entry, db: &MimeDb) -> bool {
    let file = match e.kind {
        EntryKind::File => true,
        EntryKind::Symlink => std::fs::metadata(&e.path).is_ok_and(|m| m.is_file()),
        EntryKind::Dir | EntryKind::Other => false,
    };
    file && db.by_name(&e.name).is_some_and(|m| db.is_text(m))
}

impl<R: Runtime> Kernkraft<R> {
    /// Starts a search of `roots` and everything under them for names
    /// containing `query`, ignoring case and the query's leading and
    /// trailing whitespace; dotfiles and dot-folders only with `hidden`. A
    /// query that is empty or only whitespace is refused: it would list
    /// every file. `origin` is the label of the window asking; closing it
    /// cancels the search.
    pub fn search(
        &self,
        key: String,
        roots: Vec<WirePath>,
        query: String,
        hidden: bool,
        origin: Option<String>,
    ) -> Result<OpId, WireError> {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return Err(WireError::from(PluginError::InvalidArgument {
                what: "search query",
                value: query,
            }));
        }
        self.start_scan(key, roots, hidden, ScanFilter::Name(needle), origin)
    }

    /// Starts the Documents walk of `roots`: every text file under them, by
    /// name, dotfiles left out.
    pub fn documents(
        &self,
        key: String,
        roots: Vec<WirePath>,
        origin: Option<String>,
    ) -> Result<OpId, WireError> {
        self.start_scan(key, roots, false, ScanFilter::Documents, origin)
    }

    fn start_scan(
        &self,
        key: String,
        roots: Vec<WirePath>,
        hidden: bool,
        filter: ScanFilter,
        origin: Option<String>,
    ) -> Result<OpId, WireError> {
        let roots = roots
            .into_iter()
            .map(|p| PathBuf::try_from(p).map_err(WireError::from))
            .collect::<Result<Vec<_>, _>>()?;
        // Every entry sent carries its type.
        self.mime()?;
        self.spawn(
            Job::Scan {
                key,
                roots,
                hidden,
                filter,
            },
            origin,
        )
    }

    /// Runs a walk for operation `op`, sending its batches.
    pub(crate) fn run_scan(
        &self,
        op: OpId,
        progress: &dyn Progress,
        key: String,
        roots: &[PathBuf],
        hidden: bool,
        filter: &ScanFilter,
    ) -> Result<WireScanDone, WireError> {
        let db = self.mime()?;
        let opts = ScanOptions {
            include_hidden: hidden,
            ..ScanOptions::default()
        };
        let batch: Mutex<Vec<WireEntry>> = Mutex::new(Vec::with_capacity(SEARCH_BATCH));
        let found = Mutex::new(0u64);
        let send = |entries: Vec<WireEntry>| {
            let event = SearchHitEvent {
                key: key.clone(),
                op,
                entries,
            };
            // As for every event: emit fails only on a payload serde cannot
            // serialize, and this is plain data.
            let _ = self.0.app.emit(EVENT_SEARCH_HIT, event);
        };
        let keep = |e: &Entry| {
            let hit = match filter {
                ScanFilter::Name(q) => e.name.to_lowercase().contains(q.as_str()),
                ScanFilter::Documents => is_document(e, db),
            };
            if hit {
                *lock(&found) += 1;
                let mut b = lock(&batch);
                b.push(WireEntry::new(e.clone(), db));
                if b.len() == SEARCH_BATCH {
                    send(std::mem::replace(&mut *b, Vec::with_capacity(SEARCH_BATCH)));
                }
            }
            // Never kept in the report: sent in batches instead.
            false
        };
        let report = scan(roots, &opts, &keep, progress);
        let rest = std::mem::take(&mut *lock(&batch));
        if !rest.is_empty() {
            send(rest);
        }
        let found = *lock(&found);
        Ok(WireScanDone {
            found,
            unreadable: report.unreadable.iter().map(WirePath::from).collect(),
            cancelled: report.cancelled,
        })
    }
}
