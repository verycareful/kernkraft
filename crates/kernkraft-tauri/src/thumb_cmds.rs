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

use std::collections::{HashMap, HashSet, VecDeque};
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
    path: PathBuf,
    mime: String,
    size: ThumbSize,
}

/// What the queue tracks per key, while the key has work queued or being
/// made; the entry goes when the last of it is done or cancelled, so keys
/// of closed views do not pile up.
#[derive(Default)]
struct KeyState {
    epoch: u64,
    /// Items queued plus items a worker is on.
    outstanding: usize,
    /// Labels of the windows that queued under the key.
    origins: HashSet<String>,
}

#[derive(Default)]
struct Queue {
    items: VecDeque<Item>,
    /// `(key, path, size)` of every queued item, to drop a repeated request.
    queued: HashSet<(String, PathBuf, ThumbSize)>,
    keys: HashMap<String, KeyState>,
    workers: usize,
    stopped: bool,
}

impl Queue {
    /// Queues one thumbnail. A request for a `(key, path, size)` still
    /// queued is dropped: the queued one sends the event.
    fn push(
        &mut self,
        key: &str,
        origin: Option<&str>,
        path: PathBuf,
        mime: String,
        size: ThumbSize,
    ) {
        if !self.queued.insert((key.to_owned(), path.clone(), size)) {
            return;
        }
        let state = self.keys.entry(key.to_owned()).or_default();
        state.outstanding += 1;
        if let Some(origin) = origin {
            state.origins.insert(origin.to_owned());
        }
        self.items.push_back(Item {
            key: key.to_owned(),
            epoch: state.epoch,
            path,
            mime,
            size,
        });
    }

    /// The next item; it stays outstanding until [`Queue::finish`].
    fn pop(&mut self) -> Option<Item> {
        let item = self.items.pop_front()?;
        self.queued
            .remove(&(item.key.clone(), item.path.clone(), item.size));
        Some(item)
    }

    /// Marks `item` done and says whether its event goes out: not after a
    /// cancel of its key, nor once the queue has stopped.
    fn finish(&mut self, item: &Item) -> bool {
        let Some(state) = self.keys.get_mut(&item.key) else {
            return false;
        };
        let current = state.epoch == item.epoch;
        state.outstanding = state.outstanding.saturating_sub(1);
        if state.outstanding == 0 {
            self.keys.remove(&item.key);
        }
        current && !self.stopped
    }

    /// Drops the key's queued items and suppresses the events of those in
    /// flight.
    fn cancel(&mut self, key: &str) {
        let before = self.items.len();
        self.items.retain(|i| i.key != key);
        let removed = before - self.items.len();
        self.queued.retain(|(k, _, _)| k != key);
        if let Some(state) = self.keys.get_mut(key) {
            state.epoch += 1;
            state.outstanding = state.outstanding.saturating_sub(removed);
            if state.outstanding == 0 {
                self.keys.remove(key);
            }
        }
    }

    /// Cancels every key window `label` queued under.
    fn cancel_origin(&mut self, label: &str) {
        let keys: Vec<String> = self
            .keys
            .iter()
            .filter(|(_, s)| s.origins.contains(label))
            .map(|(k, _)| k.clone())
            .collect();
        for key in keys {
            self.cancel(&key);
        }
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
    /// `"large"`, 256) under `key`, after any already queued; a path still
    /// queued at that size under that key is not queued twice. `origin` is
    /// the label of the window asking; closing it cancels its keys.
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
        for (path, mime) in typed {
            queue.push(&key, origin.as_deref(), path, mime, size);
        }
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
                    if let Some(item) = queue.pop() {
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
            let mut queue = thumbs.lock();
            if queue.finish(&item) {
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

    /// Cancels every key window `label` queued under (keys are per view, so
    /// per window), queued and in-flight items alike, or for `None` drops
    /// everything and stops the workers.
    pub(crate) fn thumbs_shutdown(&self, label: Option<&str>) {
        let thumbs = &self.0.thumbs;
        let mut queue = thumbs.lock();
        match label {
            Some(label) => queue.cancel_origin(label),
            None => {
                queue.items.clear();
                queue.queued.clear();
                queue.stopped = true;
            }
        }
        thumbs.wake.notify_all();
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::path::PathBuf;

    use kernkraft_thumb::ThumbSize;

    use super::Queue;

    fn push(q: &mut Queue, key: &str, origin: Option<&str>, path: &str) {
        q.push(
            key,
            origin,
            PathBuf::from(path),
            "image/png".to_owned(),
            ThumbSize::Normal,
        );
    }

    #[test]
    fn key_state_goes_when_its_work_is_done() {
        let mut q = Queue::default();
        push(&mut q, "k", None, "/a.png");
        push(&mut q, "k", None, "/b.png");
        let a = q.pop().unwrap();
        assert!(q.finish(&a));
        assert_eq!(q.keys.len(), 1);
        let b = q.pop().unwrap();
        assert!(q.finish(&b));
        assert!(q.keys.is_empty());
        assert!(q.queued.is_empty());
    }

    #[test]
    fn cancel_suppresses_in_flight_and_then_forgets_the_key() {
        let mut q = Queue::default();
        push(&mut q, "k", None, "/a.png");
        push(&mut q, "k", None, "/b.png");
        let in_flight = q.pop().unwrap();
        q.cancel("k");
        assert!(q.items.is_empty());
        // The key is kept while its in-flight item runs...
        assert_eq!(q.keys.len(), 1);
        // ...a new request under it is served...
        push(&mut q, "k", None, "/c.png");
        // ...and the item from before the cancel sends nothing.
        assert!(!q.finish(&in_flight));
        let c = q.pop().unwrap();
        assert!(q.finish(&c));
        assert!(q.keys.is_empty());

        // Cancelling with nothing outstanding leaves no entry behind.
        q.cancel("never");
        assert!(q.keys.is_empty());
    }

    #[test]
    fn repeated_request_still_queued_is_dropped() {
        let mut q = Queue::default();
        push(&mut q, "k", None, "/a.png");
        push(&mut q, "k", None, "/a.png");
        // Another key, or another size, is another request.
        push(&mut q, "j", None, "/a.png");
        q.push(
            "k",
            None,
            PathBuf::from("/a.png"),
            "image/png".to_owned(),
            ThumbSize::Large,
        );
        assert_eq!(q.items.len(), 3);
        assert_eq!(q.keys["k"].outstanding, 2);
        // Once popped, the same request may be queued again.
        let first = q.pop().unwrap();
        push(&mut q, "k", None, "/a.png");
        assert_eq!(q.items.len(), 3);
        assert!(q.finish(&first));
    }

    #[test]
    fn closing_a_window_cancels_its_keys_in_flight_too() {
        let mut q = Queue::default();
        push(&mut q, "grid-1", Some("main"), "/a.png");
        push(&mut q, "grid-1", Some("main"), "/b.png");
        push(&mut q, "grid-2", Some("doc-1"), "/c.png");
        let in_flight = q.pop().unwrap();
        q.cancel_origin("main");
        assert_eq!(q.items.len(), 1);
        assert_eq!(q.items[0].key, "grid-2");
        assert!(!q.finish(&in_flight));
        assert_eq!(q.keys.len(), 1);
    }
}
