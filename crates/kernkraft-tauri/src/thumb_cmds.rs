// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Thumbnails for the grid view, from the shared freedesktop cache
//! (Thumbnail Managing Standard 0.9.0, through `kernkraft-thumb`).
//!
//! [`Kernkraft::thumb_request`] queues paths under a key (one per view) and
//! returns at once. A pool of worker threads looks each one up in the cache
//! and makes it when missing, then sends [`EVENT_THUMB`] ([`ThumbEvent`]).
//! Every queued path gets exactly one event, unless its key is cancelled
//! first: [`Kernkraft::thumb_cancel`] drops the key's queued paths, and the
//! ones a worker is already on finish (the thumbnail is still cached) with
//! their event suppressed. No event of a key is sent after its cancel
//! returns.
//!
//! Only paths whose name types them as an image `kernkraft-thumb` renders
//! are queued, and the others get no event: the listing already typed them
//! by name, and a view asks for thumbnails of what it shows as images. The
//! type is never read from the content here (a folder can hold 100 000
//! entries). A file that cannot be decoded, a cached failure entry, or a
//! path inside the thumbnail cache itself (a thumbnail of a thumbnail would
//! grow the cache without end) is "no thumbnail": `file` is `null` and
//! `error` too. Only a failure to read the file or to write the cache is an
//! `error`.

use std::collections::{HashMap, VecDeque};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};

use kernkraft_thumb::{can_thumbnail, generate, lookup, Lookup, ThumbError, ThumbSize};
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Runtime};

use crate::error::{PluginError, WireError};
use crate::state::{lock, Kernkraft};
use crate::wire::WirePath;

pub const EVENT_THUMB: &str = "kernkraft://thumb";

/// Directory under the cache home that holds every thumbnail (Thumbnail
/// Managing Standard 0.9.0, "Thumbnail Directory").
const THUMBNAILS_DIR: &str = "thumbnails";

/// Payload of [`EVENT_THUMB`]: the thumbnail of `path`, requested under
/// `key`. `file` is the cached PNG, or `None` when the file has no
/// thumbnail; `error` is set, with `file` `None`, when the file or the
/// cache could not be read or written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThumbEvent {
    pub key: String,
    pub path: WirePath,
    pub file: Option<WirePath>,
    pub error: Option<WireError>,
}

struct Item {
    key: String,
    /// The key's epoch when queued; a cancel moves the key to the next one,
    /// so an item of an earlier epoch sends nothing.
    epoch: u64,
    origin: Option<String>,
    path: PathBuf,
    mime: String,
    size: ThumbSize,
}

#[derive(Default)]
struct Queue {
    items: VecDeque<Item>,
    epochs: HashMap<String, u64>,
    workers: usize,
    stopped: bool,
}

impl Queue {
    fn epoch(&self, key: &str) -> u64 {
        self.epochs.get(key).copied().unwrap_or(0)
    }

    fn cancel(&mut self, key: &str) {
        self.items.retain(|i| i.key != key);
        *self.epochs.entry(key.to_owned()).or_insert(0) += 1;
    }
}

/// The queue and its workers.
#[derive(Default)]
pub(crate) struct Thumbs {
    queue: Mutex<Queue>,
    wake: Condvar,
}

impl Thumbs {
    fn lock(&self) -> MutexGuard<'_, Queue> {
        lock(&self.queue)
    }
}

/// Worker threads: half the cores (at least one), leaving the rest for the
/// UI and file operations. Each SVG renders on its own 16 MiB thread inside
/// `kernkraft-thumb`, so this also bounds how many of those exist at once.
fn pool_size() -> usize {
    (std::thread::available_parallelism().map_or(1, NonZeroUsize::get) / 2).max(1)
}

fn parse_size(size: &str) -> Result<ThumbSize, WireError> {
    match size {
        "normal" => Ok(ThumbSize::Normal),
        "large" => Ok(ThumbSize::Large),
        other => Err(WireError::from(PluginError::InvalidArgument {
            what: "thumbnail size",
            value: other.to_owned(),
        })),
    }
}

/// Whether `path` is inside the thumbnail cache `root`, by its path as given
/// or with symlinks resolved.
fn in_cache(path: &Path, root: &Path) -> bool {
    if path.starts_with(root) {
        return true;
    }
    let real_root = root.canonicalize();
    let real = path
        .parent()
        .and_then(|p| p.canonicalize().ok())
        .map(|p| match path.file_name() {
            Some(name) => p.join(name),
            None => p,
        });
    matches!((real, real_root), (Some(p), Ok(r)) if p.starts_with(&r))
}

impl<R: Runtime> Kernkraft<R> {
    /// Queues thumbnails of `paths` at `size` (`"normal"`, 128 pixels, or
    /// `"large"`, 256) under `key`, after any already queued. `origin` is
    /// the label of the window asking; closing it drops what it queued.
    pub fn thumb_request(
        &self,
        key: String,
        paths: Vec<WirePath>,
        size: String,
        origin: Option<String>,
    ) -> Result<(), WireError> {
        let size = parse_size(&size)?;
        let paths = paths
            .into_iter()
            .map(|p| PathBuf::try_from(p).map_err(WireError::from))
            .collect::<Result<Vec<_>, _>>()?;
        self.base()?;
        let mime = self.mime()?;
        let typed: Vec<(PathBuf, String)> = paths
            .into_iter()
            .filter_map(|path| {
                let name = path.file_name()?.to_string_lossy().into_owned();
                let mime = mime.by_name(&name).filter(|m| can_thumbnail(m))?;
                Some((path, mime.to_owned()))
            })
            .collect();
        let thumbs = &self.0.thumbs;
        let mut queue = thumbs.lock();
        if queue.stopped {
            // The app is exiting: no window is left to show a thumbnail.
            return Ok(());
        }
        let epoch = queue.epoch(&key);
        queue
            .items
            .extend(typed.into_iter().map(|(path, mime)| Item {
                key: key.clone(),
                epoch,
                origin: origin.clone(),
                path,
                mime,
                size,
            }));
        self.start_workers(&mut queue)?;
        thumbs.wake.notify_all();
        Ok(())
    }

    /// Drops the queued thumbnails of `key`. Those being made finish without
    /// an event. Cancelling a key with nothing queued does nothing.
    pub fn thumb_cancel(&self, key: String) -> Result<(), WireError> {
        self.0.thumbs.lock().cancel(&key);
        Ok(())
    }

    /// Starts the pool on first use. A worker that cannot be started fails
    /// the request when none runs; with some running, the queue is served
    /// by fewer.
    fn start_workers(&self, queue: &mut Queue) -> Result<(), WireError> {
        let mut last_error = None;
        while queue.workers < pool_size() {
            let kk = self.clone();
            match std::thread::Builder::new()
                .name("kernkraft-thumb".to_owned())
                .spawn(move || kk.thumb_worker())
            {
                Ok(_) => queue.workers += 1,
                Err(e) => {
                    last_error = Some(e);
                    break;
                }
            }
        }
        match (queue.workers, last_error) {
            (0, Some(e)) => Err(WireError::from(PluginError::Internal(format!(
                "no thumbnail worker could be started: {e}"
            )))),
            _ => Ok(()),
        }
    }

    fn thumb_worker(&self) {
        let thumbs = &self.0.thumbs;
        loop {
            let item = {
                let mut queue = thumbs.lock();
                loop {
                    if queue.stopped {
                        return;
                    }
                    if let Some(item) = queue.items.pop_front() {
                        break item;
                    }
                    queue = thumbs
                        .wake
                        .wait(queue)
                        .unwrap_or_else(PoisonError::into_inner);
                }
            };
            let (file, error) = self.make_thumb(&item);
            // Sent under the queue lock, so a cancel either comes first and
            // suppresses it, or returns after it is out.
            let queue = thumbs.lock();
            if !queue.stopped && queue.epoch(&item.key) == item.epoch {
                let event = ThumbEvent {
                    key: item.key,
                    path: WirePath::from(&item.path),
                    file: file.as_ref().map(WirePath::from),
                    error,
                };
                // As for every event: emit fails only on a payload serde
                // cannot serialize, and these are plain data.
                let _ = self.0.app.emit(EVENT_THUMB, event);
            }
            drop(queue);
        }
    }

    fn make_thumb(&self, item: &Item) -> (Option<PathBuf>, Option<WireError>) {
        let cache = match self.base() {
            Ok(base) => base.cache.clone(),
            Err(e) => return (None, Some(e)),
        };
        if in_cache(&item.path, &cache.join(THUMBNAILS_DIR)) {
            return (None, None);
        }
        let made = match lookup(&cache, &item.path, item.size) {
            Ok(Lookup::Fresh(file)) => Ok(file),
            Ok(Lookup::Failed) => return (None, None),
            Ok(Lookup::Missing) => generate(&cache, &item.path, &item.mime, item.size),
            Err(e) => Err(e),
        };
        match made {
            Ok(file) => (Some(file), None),
            // Undecodable: a failure entry is cached, and there is no
            // thumbnail to show. Not an error for the user.
            Err(ThumbError::Decode { .. }) => (None, None),
            Err(e) => (None, Some(WireError::from(e))),
        }
    }

    /// Cancels every key window `label` has thumbnails queued under (keys
    /// are per view, so per window), or for `None` drops everything and
    /// stops the workers.
    pub(crate) fn thumbs_shutdown(&self, label: Option<&str>) {
        let thumbs = &self.0.thumbs;
        let mut queue = thumbs.lock();
        match label {
            Some(label) => {
                let keys: Vec<String> = queue
                    .items
                    .iter()
                    .filter(|i| i.origin.as_deref() == Some(label))
                    .map(|i| i.key.clone())
                    .collect();
                for key in keys {
                    queue.cancel(&key);
                }
            }
            None => {
                queue.items.clear();
                queue.stopped = true;
            }
        }
        thumbs.wake.notify_all();
    }
}
