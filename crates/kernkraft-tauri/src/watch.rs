// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Watching an open folder (inotify(7)) so its view stays current.
//!
//! Each watch has its own inotify instance and thread. The thread sleeps in
//! `poll(2)` on the inotify descriptor and a stop eventfd, the same pattern
//! as kernkraft-theme's watcher: dropping a [`DirWatch`] writes the eventfd
//! and joins the thread. Changes inside the folder are debounced into one
//! [`EVENT_DIR_CHANGED`]; the folder itself going away gives
//! [`EVENT_DIR_GONE`] and ends the watch.

use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use inotify::{EventMask, Inotify, WatchMask};
use rustix::event::{eventfd, poll, EventfdFlags, PollFd, PollFlags, Timespec};
use rustix::io::Errno;
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Runtime};

use crate::error::{PluginError, WireError};
use crate::state::{lock, Kernkraft};
use crate::wire::WirePath;

pub const EVENT_DIR_CHANGED: &str = "kernkraft://dir-changed";
pub const EVENT_DIR_GONE: &str = "kernkraft://dir-gone";

/// Quiet time after the last change before [`EVENT_DIR_CHANGED`] fires: a
/// bulk copy into the open folder fires one inotify event per file, and
/// the view wants one refresh per burst.
pub const DEBOUNCE_MS: u64 = 200;

/// A burst that never goes quiet (a long copy into the folder) still
/// refreshes the view this often, so it does not sit stale until the copy
/// ends.
const MAX_DEFER: Duration = Duration::from_secs(1);

/// Read buffer for inotify events: inotify(7) says a buffer of
/// `sizeof(struct inotify_event) + NAME_MAX + 1` holds at least one event;
/// this holds several.
const EVENT_BUF: usize = 4096;

/// Payload of [`EVENT_DIR_CHANGED`] and [`EVENT_DIR_GONE`]: the key the
/// watch was started under. `error` is set only on a [`EVENT_DIR_GONE`]
/// that ends the watch because the watch itself failed, with the folder
/// possibly still there: the view lists it again to find out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirEvent {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<WireError>,
}

/// Changes that alter a folder's listing: entries added, removed or renamed,
/// written, or changed in mode or times. `ONLYDIR` refuses a path that is
/// not a folder. The `*_SELF` events report the folder itself going away;
/// the kernel adds `IN_UNMOUNT` and `IN_IGNORED` to every watch.
fn watch_mask() -> WatchMask {
    WatchMask::CREATE
        | WatchMask::DELETE
        | WatchMask::MOVED_FROM
        | WatchMask::MOVED_TO
        | WatchMask::MODIFY
        | WatchMask::CLOSE_WRITE
        | WatchMask::ATTRIB
        | WatchMask::DELETE_SELF
        | WatchMask::MOVE_SELF
        | WatchMask::ONLYDIR
}

/// The folder was deleted or moved away (the watch follows the inode, so a
/// moved folder is no longer at the path the view shows), its filesystem
/// was unmounted, or the kernel ended the watch.
fn gone(mask: EventMask) -> bool {
    mask.intersects(
        EventMask::DELETE_SELF | EventMask::MOVE_SELF | EventMask::UNMOUNT | EventMask::IGNORED,
    )
}

/// A running folder watch. Dropping it stops the thread and waits for it.
pub(crate) struct DirWatch {
    /// Label of the window that asked for it, if any.
    pub origin: Option<String>,
    stop: Arc<AtomicBool>,
    stop_fd: Arc<OwnedFd>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for DirWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Waking the thread: an eventfd takes an 8-byte counter increment.
        // The write fails only if the counter would overflow, which a
        // handful of stops cannot reach.
        let _ = rustix::io::write(&*self.stop_fd, &1u64.to_ne_bytes());
        if let Some(t) = self.thread.take() {
            if t.thread().id() != thread::current().id() {
                // The thread only sleeps in poll or emits an event; it
                // returns as soon as it sees the stop.
                let _ = t.join();
            }
        }
    }
}

impl<R: Runtime> Kernkraft<R> {
    /// Watches folder `path` under `key`, replacing any watch already
    /// under that key. `origin` is the label of the window asking; closing
    /// that window ends the watch.
    pub fn watch_dir(
        &self,
        key: String,
        path: WirePath,
        origin: Option<String>,
    ) -> Result<(), WireError> {
        let dir = PathBuf::try_from(path)?;
        std::fs::metadata(&dir).map_err(|e| {
            WireError::from(if e.kind() == std::io::ErrorKind::NotFound {
                kernkraft_fs::FsError::NotFound { path: dir.clone() }
            } else {
                kernkraft_fs::FsError::Io {
                    path: dir.clone(),
                    source: e,
                }
            })
        })?;
        let watch = self.start_watch(key.clone(), &dir, origin)?;
        let old = lock(&self.0.watches).insert(key, watch);
        // Dropped outside the lock: the drop waits for its thread.
        drop(old);
        Ok(())
    }

    /// Ends the watch under `key`. A watch whose folder went away stays
    /// registered until this is called.
    pub fn unwatch_dir(&self, key: &str) -> Result<(), WireError> {
        let old = lock(&self.0.watches).remove(key);
        match old {
            Some(watch) => {
                drop(watch);
                Ok(())
            }
            None => Err(WireError::from(PluginError::UnknownWatch(key.to_owned()))),
        }
    }

    fn start_watch(
        &self,
        key: String,
        dir: &Path,
        origin: Option<String>,
    ) -> Result<DirWatch, WireError> {
        let err = |source: std::io::Error| {
            WireError::from(PluginError::Watch {
                path: dir.to_path_buf(),
                source,
            })
        };
        let inotify = Inotify::init().map_err(err)?;
        inotify.watches().add(dir, watch_mask()).map_err(err)?;
        let stop_fd =
            Arc::new(eventfd(0, EventfdFlags::CLOEXEC).map_err(|e| err(std::io::Error::from(e)))?);
        let stop = Arc::new(AtomicBool::new(false));
        let kk = self.clone();
        let (tstop, tfd, tdir) = (stop.clone(), stop_fd.clone(), dir.to_path_buf());
        let thread = thread::Builder::new()
            // A fixed name: the key comes from the webview and could hold
            // a NUL, which a thread name cannot.
            .name("kernkraft-watch".to_owned())
            .spawn(move || watch_loop(&kk, &key, &tdir, inotify, &tfd, &tstop))
            .map_err(err)?;
        Ok(DirWatch {
            origin,
            stop,
            stop_fd,
            thread: Some(thread),
        })
    }
}

fn watch_loop<R: Runtime>(
    kk: &Kernkraft<R>,
    key: &str,
    dir: &Path,
    mut inotify: Inotify,
    stop_fd: &OwnedFd,
    stop: &AtomicBool,
) {
    let emit = |event: &str, error: Option<std::io::Error>| {
        let error = error.map(|source| {
            WireError::from(PluginError::Watch {
                path: dir.to_path_buf(),
                source,
            })
        });
        // As for operation events, emit fails only for an invalid name or
        // payload, and these are fixed.
        let _ = kk.0.app.emit(
            event,
            DirEvent {
                key: key.to_owned(),
                error,
            },
        );
    };
    let send = |event: &str| emit(event, None);
    let mut buf = [0u8; EVENT_BUF];
    // First and latest change of the burst not yet reported.
    let mut burst: Option<(Instant, Instant)> = None;
    loop {
        let deadline = burst.map(|(first, last)| {
            (last + Duration::from_millis(DEBOUNCE_MS)).min(first + MAX_DEFER)
        });
        let now = Instant::now();
        if let Some(d) = deadline {
            if d <= now {
                burst = None;
                send(EVENT_DIR_CHANGED);
                continue;
            }
        }
        let timeout = deadline.and_then(|d| Timespec::try_from(d - now).ok());
        let mut fds = [
            PollFd::new(&inotify, PollFlags::IN),
            PollFd::new(stop_fd, PollFlags::IN),
        ];
        match poll(&mut fds, timeout.as_ref()) {
            Ok(_) => {}
            Err(Errno::INTR) => continue,
            // poll fails only for resource exhaustion. The view can no
            // longer be kept current: the watch ends and says why.
            Err(e) => {
                emit(EVENT_DIR_GONE, Some(e.into()));
                return;
            }
        }
        if stop.load(Ordering::SeqCst) || !fds[1].revents().is_empty() {
            return;
        }
        if fds[0].revents().is_empty() {
            continue;
        }
        match inotify.read_events(&mut buf) {
            Ok(events) => {
                let mut changed = false;
                for event in events {
                    if gone(event.mask) {
                        send(EVENT_DIR_GONE);
                        return;
                    }
                    // Q_OVERFLOW: events were lost, so the listing may be
                    // stale; it counts as a change.
                    changed = true;
                }
                if changed {
                    let now = Instant::now();
                    burst = Some(burst.map_or((now, now), |(first, _)| (first, now)));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => {
                emit(EVENT_DIR_GONE, Some(e));
                return;
            }
        }
    }
}
