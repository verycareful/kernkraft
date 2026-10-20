// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The trash view: list, restore and purge (FreeDesktop Trash 1.0).
//!
//! An item the webview names is acted on only if it matches, field for
//! field, an item the trash lists at that moment. The webview is untrusted:
//! a forged name such as `../x` would otherwise point a purge outside the
//! trash.

use std::path::PathBuf;

use kernkraft_fs::{OpReport, TrashedItem};
use serde::{Deserialize, Serialize};
use tauri::Runtime;

use crate::error::{PluginError, WireError, WireItemError};
use crate::fs_cmds::describe;
use crate::ops::{held, WireReport};
use crate::state::Kernkraft;
use crate::wire::WirePath;

/// One item in a trash. `deleted` is the raw `DeletionDate` value,
/// `YYYY-MM-DDThh:mm:ss` in local time; `trash_dir` is the trash directory
/// holding the item under `files/<name>`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireTrashedItem {
    pub name: String,
    pub original: WirePath,
    pub deleted: String,
    pub trash_dir: WirePath,
    pub is_dir: bool,
}

impl From<&TrashedItem> for WireTrashedItem {
    fn from(item: &TrashedItem) -> WireTrashedItem {
        WireTrashedItem {
            name: item.name.clone(),
            original: WirePath::from(&item.original),
            deleted: item.deleted.clone(),
            trash_dir: WirePath::from(&item.trash_dir),
            is_dir: item.is_dir,
        }
    }
}

/// Every item in the home trash and the trashes of the mounted volumes,
/// and each info file that could not be read, with the reason.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireTrashListing {
    pub items: Vec<WireTrashedItem>,
    pub unreadable: Vec<WireItemError>,
}

impl<R: Runtime> Kernkraft<R> {
    pub fn trash_list(&self) -> Result<WireTrashListing, WireError> {
        let listing = self.trash()?.list()?;
        Ok(WireTrashListing {
            items: listing.items.iter().map(WireTrashedItem::from).collect(),
            unreadable: listing
                .unreadable
                .iter()
                .map(|(p, e)| WireItemError::new(p, e))
                .collect(),
        })
    }

    /// Moves each item back to where it was trashed from, recreating a
    /// missing parent folder and never replacing anything there. Each
    /// `done` entry is the item's path in the trash and the path it is back
    /// at. Not journalled: undo reverses file operations, and restoring is
    /// itself the reversal of one.
    pub fn trash_restore(&self, items: Vec<WireTrashedItem>) -> Result<WireReport, WireError> {
        let trash = self.trash()?;
        self.run_registered(|| {
            let (found, rejected) = self.match_listed(&items)?;
            let mut report = OpReport::default();
            for item in found {
                match trash.restore(&item) {
                    Ok(back) => report.done.push((held(&item), Some(back))),
                    Err(e) => report.failed.push((item.original.clone(), e)),
                }
            }
            Ok(with_rejected(&report, rejected))
        })
    }

    /// Deletes each item from the trash for good. Each `done` entry is the
    /// item's original path. The next undo says that this cannot be undone.
    pub fn trash_purge(&self, items: Vec<WireTrashedItem>) -> Result<WireReport, WireError> {
        let trash = self.trash()?;
        self.run_registered(|| {
            let (found, rejected) = self.match_listed(&items)?;
            let mut report = OpReport::default();
            for item in found {
                match trash.purge(&item) {
                    Ok(()) => report.done.push((item.original.clone(), None)),
                    Err(e) => report.failed.push((item.original.clone(), e)),
                }
            }
            self.record_irreversible(&report, || {
                let purged: Vec<PathBuf> = report.done.iter().map(|(p, _)| p.clone()).collect();
                format!("delete from trash of {}", describe(&purged))
            });
            Ok(with_rejected(&report, rejected))
        })
    }

    /// The listed items `items` name, and a `NotInTrash` failure for each
    /// one that names no listed item.
    fn match_listed(
        &self,
        items: &[WireTrashedItem],
    ) -> Result<(Vec<TrashedItem>, Vec<WireItemError>), WireError> {
        let listed = self.trash()?.list()?.items;
        let mut found = Vec::new();
        let mut rejected = Vec::new();
        for wire in items {
            match listed
                .iter()
                .find(|item| WireTrashedItem::from(*item) == *wire)
            {
                Some(item) => found.push(item.clone()),
                None => {
                    // Shown only, never acted on: a path that does not
                    // decode is shown as its lossy text.
                    let original = PathBuf::try_from(wire.original.clone())
                        .unwrap_or_else(|_| PathBuf::from(wire.original.display()));
                    rejected.push(WireItemError {
                        path: wire.original.clone(),
                        error: WireError::from(PluginError::NotInTrash(original)),
                    });
                }
            }
        }
        Ok((found, rejected))
    }
}

/// `report` on the wire, with the rejected items first among the failures.
fn with_rejected(report: &OpReport, rejected: Vec<WireItemError>) -> WireReport {
    let mut wire = WireReport::new(report, &[]);
    wire.failed.splice(0..0, rejected);
    wire
}
