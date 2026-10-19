// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Live theme watching (parent spec section 6.5): file changes under the
//! config directories through inotify (inotify(7)), and the portal's
//! `SettingChanged` signal (xdg-desktop-portal Settings interface) on the
//! session bus.
//!
//! Threads:
//! - the reader sleeps in `poll(2)` on the inotify descriptor and a stop
//!   eventfd, reads events (the inotify descriptor is non-blocking) and
//!   forwards them;
//! - the worker owns the watches, debounces, gathers and calls back;
//! - the portal thread reads `SettingChanged` signals and forwards them.
//!
//! Dropping a [`Watcher`] writes the eventfd (the reader wakes and exits),
//! stops the worker, closes the bus connection (which ends the portal
//! thread's blocking read) and waits for each thread, at most
//! [`DROP_WAIT`] each.

use std::collections::HashMap;
use std::ffi::OsString;
use std::os::fd::OwnedFd;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask, Watches};
use kernkraft_fs::BaseDirs;
use rustix::event::{eventfd, poll, EventfdFlags, PollFd, PollFlags};
use rustix::io::Errno;
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Type as MessageType;
use zbus::MatchRule;

use crate::error::ThemeError;
use crate::gather::{gather_from, Gathered, GTK_DIR, HYPR_DIR, HYPR_FILE};
use crate::portal::{APPEARANCE_NAMESPACE, PORTAL_BUS_NAME, PORTAL_PATH, SETTINGS_IFACE};

/// Quiet time after the last change before one re-theme runs. The dotfiles
/// `theme` script rewrites several files in sequence; waiting for the burst
/// to end gives one re-theme per burst instead of one per file.
pub const DEBOUNCE: Duration = Duration::from_millis(150);

/// A burst that never goes quiet is flushed after this long, so a writer
/// that touches a watched file continuously cannot hold the theme back
/// forever.
const MAX_DEFER: Duration = Duration::from_secs(2);

/// Longest [`Watcher`]'s drop waits for one thread. A worker is busy only
/// while gathering (bounded by the bus method timeout per portal call) or in
/// the caller's callback; a callback that never returns would otherwise hang
/// the drop. Past this the thread is left to finish on its own: it has been
/// told to stop and exits as soon as it returns.
pub const DROP_WAIT: Duration = Duration::from_secs(10);

/// Events that can change what the theme reads. `MODIFY` is included for
/// writers that keep the file open. The `*_SELF` and ignored events report
/// the watched directory itself going away.
fn watch_mask() -> WatchMask {
    WatchMask::CREATE
        | WatchMask::DELETE
        | WatchMask::MOVED_FROM
        | WatchMask::MOVED_TO
        | WatchMask::CLOSE_WRITE
        | WatchMask::MODIFY
        | WatchMask::DELETE_SELF
        | WatchMask::MOVE_SELF
}

/// The files the watcher reacts to, per watched directory.
struct Target {
    dir: PathBuf,
    files: &'static [&'static str],
    /// The directory actually under watch: `dir`, or its nearest existing
    /// ancestor while `dir` does not exist yet.
    watched: Option<PathBuf>,
}

/// One inotify event, reduced to what the worker needs.
struct RawEvent {
    wd: WatchDescriptor,
    mask: EventMask,
    name: Option<OsString>,
}

enum Msg {
    Fs(Vec<RawEvent>),
    FsError(std::io::Error),
    Portal,
    PortalError(ThemeError),
    Stop,
}

/// A thread plus a signal that it has finished.
struct Joinable {
    handle: JoinHandle<()>,
    done: mpsc::Receiver<()>,
}

/// Sends on drop, so the signal arrives even when the thread unwinds.
struct Done(Sender<()>);
impl Drop for Done {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

fn spawn_joinable(f: impl FnOnce() + Send + 'static) -> Joinable {
    let (tx, done) = mpsc::channel();
    let handle = thread::spawn(move || {
        let _done = Done(tx);
        f();
    });
    Joinable { handle, done }
}

impl Joinable {
    /// Waits up to [`DROP_WAIT`] for the thread; past that it is detached.
    /// Skipped for the calling thread itself (a drop inside the callback).
    fn finish(self) {
        if self.handle.thread().id() == thread::current().id() {
            return;
        }
        match self.done.recv_timeout(DROP_WAIT) {
            Err(RecvTimeoutError::Timeout) => {}
            _ => {
                let _ = self.handle.join();
            }
        }
    }
}

/// A running watch. Dropping it stops the file watch and the portal
/// subscription and waits for the threads (see [`DROP_WAIT`]).
///
/// A panic in the `on_change` callback is caught: the watcher keeps running
/// and the panic message is reported as [`ThemeError::CallbackPanic`] in the
/// errors of the next callback.
pub struct Watcher {
    tx: Sender<Msg>,
    stop: Arc<AtomicBool>,
    stop_fd: Arc<OwnedFd>,
    reader: Option<Joinable>,
    worker: Option<Joinable>,
    portal: Option<Joinable>,
    conn: Option<Connection>,
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Waking the reader: an eventfd takes an 8-byte counter increment.
        let _ = rustix::io::write(&*self.stop_fd, &1u64.to_ne_bytes());
        // A send fails only when the worker already ended.
        let _ = self.tx.send(Msg::Stop);
        if let Some(j) = self.reader.take() {
            j.finish();
        }
        if let Some(j) = self.worker.take() {
            j.finish();
        }
        if let Some(conn) = self.conn.take() {
            // Closing the connection ends the portal thread's blocking
            // read.
            let _ = conn.close();
        }
        if let Some(j) = self.portal.take() {
            j.finish();
        }
    }
}

/// Watches the theme sources under `base.config` and the portal on the
/// session bus, calling `on_change` with a fresh [`Gathered`] after each
/// burst of changes. See [`watch_with`] for the details; it applies here
/// unchanged.
///
/// The bus is reached over its Unix socket only (see `kernkraft-bus`).
/// Only a bus that is absent (no address could be built, or the default
/// socket does not exist) leaves the portal out and watches files alone.
/// Every other failure, including an address the environment names that
/// cannot be reached or is not a Unix address, is an error.
pub fn watch(
    base: &BaseDirs,
    on_change: Box<dyn Fn(Gathered) + Send>,
) -> Result<Watcher, ThemeError> {
    let conn = match kernkraft_bus::session() {
        Ok(c) => Some(c),
        Err(e) if e.is_absent() => None,
        Err(e) => return Err(ThemeError::Portal(format!("session bus: {e}"))),
    };
    watch_with(base, conn, on_change)
}

/// [`watch`] on a given connection (`None` skips the portal).
///
/// Nothing is delivered for the state at the start: call [`gather`] once
/// first, then use the callbacks for changes.
///
/// Watches `hypr` and `gtk-4.0` under `base.config` as directories, because
/// editors and the theme script replace files by rename and a watch on the
/// file's inode would be lost. When a directory does not exist yet, its
/// nearest existing ancestor is watched until it appears. Changes to
/// `hyprtoolkit.conf`, `settings.ini` and `gtk.css`, and portal changes in
/// the appearance namespace, are debounced by [`DEBOUNCE`]; then the sources
/// are gathered and `on_change` runs on the worker thread. Errors from the
/// watch itself arrive in [`Gathered::errors`] of the next callback.
///
/// If `on_change` panics, the panic is caught, the watcher keeps running,
/// and the next callback's errors carry a [`ThemeError::CallbackPanic`] with
/// the message. If no further change arrives, that report is never delivered.
///
/// [`gather`]: crate::gather
pub fn watch_with(
    base: &BaseDirs,
    conn: Option<Connection>,
    on_change: Box<dyn Fn(Gathered) + Send>,
) -> Result<Watcher, ThemeError> {
    let config = base.config.clone();
    let io_err = |path: &Path, source| ThemeError::Watch {
        path: path.to_path_buf(),
        source,
    };
    let inotify = Inotify::init().map_err(|e| io_err(&config, e))?;
    let watches = inotify.watches();
    let stop_fd = Arc::new(
        eventfd(0, EventfdFlags::CLOEXEC).map_err(|e| io_err(&config, std::io::Error::from(e)))?,
    );

    let (tx, rx) = mpsc::channel::<Msg>();
    let stop = Arc::new(AtomicBool::new(false));

    let mut state = Watching {
        watches,
        by_wd: HashMap::new(),
        by_path: HashMap::new(),
    };
    let mut targets = vec![
        Target {
            dir: config.join(HYPR_DIR),
            files: &[HYPR_FILE],
            watched: None,
        },
        Target {
            dir: config.join(GTK_DIR),
            files: &["settings.ini", "gtk.css"],
            watched: None,
        },
    ];
    state.arm(&mut targets, false)?;

    let mut portal = None;
    if let Some(c) = &conn {
        let rule = MatchRule::builder()
            .msg_type(MessageType::Signal)
            .sender(PORTAL_BUS_NAME)
            .and_then(|b| b.interface(SETTINGS_IFACE))
            .and_then(|b| b.member("SettingChanged"))
            .and_then(|b| b.path(PORTAL_PATH))
            .and_then(|b| b.arg(0, APPEARANCE_NAMESPACE))
            .map(|b| b.build())
            .map_err(|e| ThemeError::Portal(format!("match rule: {e}")))?;
        // The match is registered with the bus daemon, not the portal, so
        // it succeeds with no portal running and delivers once one starts.
        let iter = MessageIterator::for_match_rule(rule, c, None)
            .map_err(|e| ThemeError::Portal(format!("subscribe: {e}")))?;
        let (ptx, pstop) = (tx.clone(), stop.clone());
        portal = Some(spawn_joinable(move || {
            for msg in iter {
                match msg {
                    Ok(_) => {
                        if ptx.send(Msg::Portal).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        if !pstop.load(Ordering::SeqCst) {
                            let _ = ptx.send(Msg::PortalError(ThemeError::Portal(format!(
                                "SettingChanged: {e}"
                            ))));
                        }
                        break;
                    }
                }
            }
        }));
    }

    let reader = {
        let (rtx, rstop_fd, rstop) = (tx.clone(), stop_fd.clone(), stop.clone());
        spawn_joinable(move || read_loop(inotify, &rstop_fd, &rstop, &rtx))
    };

    let worker_conn = conn.clone();
    let worker = spawn_joinable(move || {
        run(&config, state, targets, worker_conn, rx, on_change);
    });

    Ok(Watcher {
        tx,
        stop,
        stop_fd,
        reader: Some(reader),
        worker: Some(worker),
        portal,
        conn,
    })
}

/// Sleeps until the inotify descriptor or the stop eventfd is readable,
/// forwards events, and exits when stopped or on a read error.
fn read_loop(mut inotify: Inotify, stop_fd: &OwnedFd, stop: &AtomicBool, tx: &Sender<Msg>) {
    let mut buf = [0u8; 4096];
    loop {
        let mut fds = [
            PollFd::new(&inotify, PollFlags::IN),
            PollFd::new(stop_fd, PollFlags::IN),
        ];
        match poll(&mut fds, None) {
            Ok(_) => {}
            Err(Errno::INTR) => continue,
            Err(e) => {
                let _ = tx.send(Msg::FsError(e.into()));
                return;
            }
        }
        if stop.load(Ordering::SeqCst) || !fds[1].revents().is_empty() {
            return;
        }
        match inotify.read_events(&mut buf) {
            Ok(events) => {
                let batch: Vec<RawEvent> = events
                    .map(|e| RawEvent {
                        wd: e.wd,
                        mask: e.mask,
                        name: e.name.map(OsString::from),
                    })
                    .collect();
                if !batch.is_empty() && tx.send(Msg::Fs(batch)).is_err() {
                    return;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => {
                let _ = tx.send(Msg::FsError(e));
                return;
            }
        }
    }
}

/// The directory to watch for `dir`: itself, or the nearest existing
/// ancestor.
fn nearest_existing(dir: &Path) -> PathBuf {
    let mut p = dir;
    while !p.is_dir() {
        match p.parent() {
            Some(parent) => p = parent,
            None => break,
        }
    }
    p.to_path_buf()
}

/// The live inotify watches and the maps between descriptors and paths.
struct Watching {
    watches: Watches,
    by_wd: HashMap<WatchDescriptor, PathBuf>,
    by_path: HashMap<PathBuf, WatchDescriptor>,
}

impl Watching {
    /// Points every target's watch at its directory or nearest existing
    /// ancestor, adding new watches and dropping ones no target uses. With
    /// `force`, every watch is replaced, for when a watched directory was
    /// removed and recreated (the kernel ended the old watch with the old
    /// inode).
    fn arm(&mut self, targets: &mut [Target], force: bool) -> Result<(), ThemeError> {
        if force {
            let all: Vec<PathBuf> = self.by_path.keys().cloned().collect();
            for p in all {
                self.remove(&p)?;
            }
            for t in targets.iter_mut() {
                t.watched = None;
            }
        }
        for i in 0..targets.len() {
            // A directory can appear between choosing the ancestor and the
            // watch being in place, so look again after adding and move
            // down until the choice is stable.
            loop {
                let want = nearest_existing(&targets[i].dir);
                let old = targets[i].watched.clone();
                if old.as_ref() == Some(&want) && self.by_path.contains_key(&want) {
                    break;
                }
                if !self.by_path.contains_key(&want) {
                    let wd = self.watches.add(&want, watch_mask()).map_err(|source| {
                        ThemeError::Watch {
                            path: want.clone(),
                            source,
                        }
                    })?;
                    self.by_wd.insert(wd.clone(), want.clone());
                    self.by_path.insert(want.clone(), wd);
                }
                targets[i].watched = Some(want.clone());
                if let Some(old) = old.filter(|o| *o != want) {
                    if !targets.iter().any(|t| t.watched.as_ref() == Some(&old)) {
                        self.remove(&old)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn remove(&mut self, path: &Path) -> Result<(), ThemeError> {
        let Some(wd) = self.by_path.remove(path) else {
            return Ok(());
        };
        self.by_wd.remove(&wd);
        match self.watches.remove(wd) {
            Ok(()) => Ok(()),
            // The kernel already ended the watch (the directory was
            // removed).
            Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => Ok(()),
            Err(source) => Err(ThemeError::Watch {
                path: path.to_path_buf(),
                source,
            }),
        }
    }
}

/// Whether the event at `path` touches a file the theme reads, or the path
/// to a watched directory that does not exist yet.
fn relevant(path: &Path, targets: &[Target]) -> bool {
    // The directory-level events carry no name, and then `path` is the
    // watched directory itself.
    targets.iter().any(|t| {
        // `path` is the directory or one of its ancestors (the directory
        // appeared, was removed, or its parent changed).
        t.dir.starts_with(path)
            || (path.parent() == Some(t.dir.as_path())
                && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| t.files.contains(&n)))
    })
}

/// What one burst of messages amounted to.
#[derive(Default)]
struct Burst {
    /// Something the theme reads changed.
    pending: bool,
    /// A watched directory or ancestor was created, removed or renamed, or
    /// events were lost.
    rearm: bool,
}

/// Folds `msg` into `burst`. Returns whether it counted as a change (which
/// restarts the quiet period), or `None` for [`Msg::Stop`].
fn absorb(
    msg: Msg,
    config: &Path,
    state: &Watching,
    targets: &[Target],
    errors: &mut Vec<ThemeError>,
    burst: &mut Burst,
) -> Option<bool> {
    let changed = match msg {
        Msg::Stop => return None,
        Msg::Fs(events) => {
            let mut any = false;
            for ev in events {
                if ev.mask.contains(EventMask::Q_OVERFLOW) {
                    // Events were lost: re-read everything and re-arm.
                    burst.rearm = true;
                    any = true;
                    continue;
                }
                let Some(dir) = state.by_wd.get(&ev.wd) else {
                    continue;
                };
                let path = match &ev.name {
                    Some(n) => dir.join(n),
                    None => dir.clone(),
                };
                if !relevant(&path, targets) {
                    continue;
                }
                any = true;
                if ev.mask.intersects(
                    EventMask::CREATE
                        | EventMask::DELETE
                        | EventMask::MOVED_FROM
                        | EventMask::MOVED_TO
                        | EventMask::DELETE_SELF
                        | EventMask::MOVE_SELF
                        | EventMask::IGNORED,
                ) && targets.iter().any(|t| t.dir.starts_with(&path))
                {
                    burst.rearm = true;
                }
            }
            any
        }
        Msg::FsError(source) => {
            errors.push(ThemeError::Watch {
                path: config.to_path_buf(),
                source,
            });
            true
        }
        Msg::Portal => true,
        Msg::PortalError(e) => {
            errors.push(e);
            true
        }
    };
    burst.pending |= changed;
    Some(changed)
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

fn run(
    config: &Path,
    mut state: Watching,
    mut targets: Vec<Target>,
    conn: Option<Connection>,
    rx: mpsc::Receiver<Msg>,
    on_change: Box<dyn Fn(Gathered) + Send>,
) {
    let mut errors: Vec<ThemeError> = Vec::new();
    loop {
        let mut burst = Burst::default();
        // Wait for the first change of a burst.
        let Ok(first) = rx.recv() else { return };
        if absorb(first, config, &state, &targets, &mut errors, &mut burst).is_none() {
            return;
        }
        if !burst.pending {
            continue;
        }
        // Debounce: flush after DEBOUNCE of quiet, or MAX_DEFER in all.
        let started = Instant::now();
        let mut quiet_since = Instant::now();
        loop {
            let wait = (quiet_since + DEBOUNCE)
                .min(started + MAX_DEFER)
                .saturating_duration_since(Instant::now());
            if wait.is_zero() {
                break;
            }
            match rx.recv_timeout(wait) {
                Ok(m) => match absorb(m, config, &state, &targets, &mut errors, &mut burst) {
                    None => return,
                    Some(true) => quiet_since = Instant::now(),
                    Some(false) => {}
                },
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        if let Err(e) = state.arm(&mut targets, burst.rearm) {
            errors.push(e);
        }
        let mut gathered = gather_from(config, conn.as_ref());
        gathered.errors.append(&mut errors);
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| on_change(gathered))) {
            errors.push(ThemeError::CallbackPanic(panic_message(&*payload)));
        }
    }
}
