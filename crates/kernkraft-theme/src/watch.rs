// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Live theme watching (parent spec section 6.5): file changes under the
//! config directories through inotify (`notify`), and the portal's
//! `SettingChanged` signal (xdg-desktop-portal Settings interface) on the
//! session bus.
//!
//! Threads: one worker owns the `notify` watcher, debounces, gathers and
//! calls back; one thread reads portal signals and forwards them to the
//! worker. Dropping a [`Watcher`] stops and joins both. The portal thread
//! sits in a blocking read of the bus connection; dropping closes that
//! connection, which ends the read.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use kernkraft_fs::BaseDirs;
use notify::event::{AccessKind, AccessMode, EventKind};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher as _};
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Type as MessageType;
use zbus::MatchRule;

use crate::error::ThemeError;
use crate::gather::{gather_from, Gathered, GTK_DIR, HYPR_DIR, HYPR_FILE};
use crate::portal::{
    is_no_portal, APPEARANCE_NAMESPACE, PORTAL_BUS_NAME, PORTAL_PATH, SETTINGS_IFACE,
};

/// Quiet time after the last change before one re-theme runs. The dotfiles
/// `theme` script rewrites several files in sequence; waiting for the burst
/// to end gives one re-theme per burst instead of one per file.
pub const DEBOUNCE: Duration = Duration::from_millis(150);

/// A burst that never goes quiet is flushed after this long, so a writer
/// that touches a watched file continuously cannot hold the theme back
/// forever.
const MAX_DEFER: Duration = Duration::from_secs(2);

/// The files the watcher reacts to, per watched directory.
struct Target {
    dir: PathBuf,
    files: &'static [&'static str],
    /// The directory actually under watch: `dir`, or its nearest existing
    /// ancestor while `dir` does not exist yet.
    watched: Option<PathBuf>,
}

enum Msg {
    Fs(notify::Result<Event>),
    Portal,
    PortalError(ThemeError),
    Stop,
}

/// A running watch. Dropping it stops the file watch and the portal
/// subscription and waits for both threads to finish (from any thread other
/// than the callback's own; dropping inside the callback stops the threads
/// without waiting for itself).
pub struct Watcher {
    tx: Sender<Msg>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    portal: Option<JoinHandle<()>>,
    conn: Option<Connection>,
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // A send fails only when the worker already ended.
        let _ = self.tx.send(Msg::Stop);
        let me = thread::current().id();
        if let Some(h) = self.worker.take() {
            if h.thread().id() != me {
                let _ = h.join();
            }
        }
        if let Some(conn) = self.conn.take() {
            // Closing the connection ends the portal thread's blocking
            // read. The worker is gone, so nothing else uses it.
            let _ = conn.close();
        }
        if let Some(h) = self.portal.take() {
            if h.thread().id() != me {
                let _ = h.join();
            }
        }
    }
}

/// Watches the theme sources under `base.config` and the portal on the
/// session bus, calling `on_change` with a fresh [`Gathered`] after each
/// burst of changes. See [`watch_with`].
///
/// The bus is reached over its Unix socket only (see `kernkraft-bus`).
/// Without a session bus the file watch still runs and the portal is left
/// out, the same as a bus with no portal on it. Any other failure to reach
/// the bus is an error.
pub fn watch(
    base: &BaseDirs,
    on_change: Box<dyn Fn(Gathered) + Send>,
) -> Result<Watcher, ThemeError> {
    // Only a missing bus is tolerated; every other failure, including a
    // non-Unix address, is reported.
    let conn = match kernkraft_bus::session() {
        Ok(c) => Some(c),
        Err(kernkraft_bus::BusError::NoAddress(_)) => None,
        Err(kernkraft_bus::BusError::Connect { source, .. })
            if matches!(
                source.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            None
        }
        Err(e) => return Err(ThemeError::Portal(format!("session bus: {e}"))),
    };
    watch_with(base, conn, on_change)
}

/// [`watch`] on a given connection (`None` skips the portal).
///
/// Watches `hypr` and `gtk-4.0` under `base.config` as directories, because
/// editors and the theme script replace files by rename and a watch on the
/// file's inode would be lost. When a directory does not exist yet, its
/// nearest existing ancestor is watched until it appears. Changes to
/// `hyprtoolkit.conf`, `settings.ini` and `gtk.css`, and portal changes in
/// the appearance namespace, are debounced by [`DEBOUNCE`]; then the sources
/// are gathered and `on_change` runs on the worker thread. Errors from the
/// watch itself arrive in [`Gathered::errors`] of the next callback.
pub fn watch_with(
    base: &BaseDirs,
    conn: Option<Connection>,
    on_change: Box<dyn Fn(Gathered) + Send>,
) -> Result<Watcher, ThemeError> {
    let config = base.config.clone();
    let (tx, rx) = mpsc::channel::<Msg>();
    let stop = Arc::new(AtomicBool::new(false));

    let fs_tx = tx.clone();
    let mut notifier = notify::recommended_watcher(move |res| {
        let _ = fs_tx.send(Msg::Fs(res));
    })
    .map_err(|source| ThemeError::Watch {
        path: config.clone(),
        source,
    })?;

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
    arm(&mut notifier, &mut targets, false)?;

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
        match MessageIterator::for_match_rule(rule, c, None) {
            Ok(iter) => {
                let (ptx, pstop) = (tx.clone(), stop.clone());
                portal = Some(thread::spawn(move || {
                    for msg in iter {
                        match msg {
                            Ok(_) => {
                                if ptx.send(Msg::Portal).is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                if !pstop.load(Ordering::SeqCst) {
                                    let _ = ptx.send(Msg::PortalError(ThemeError::Portal(
                                        format!("SettingChanged: {e}"),
                                    )));
                                }
                                break;
                            }
                        }
                    }
                }));
            }
            // No portal on the bus: nothing to subscribe to.
            Err(e) if is_no_portal(&e) => {}
            Err(e) => return Err(ThemeError::Portal(format!("subscribe: {e}"))),
        }
    }

    let worker_conn = conn.clone();
    let worker = thread::spawn(move || {
        run(&config, notifier, targets, worker_conn, rx, on_change);
    });

    Ok(Watcher {
        tx,
        stop,
        worker: Some(worker),
        portal,
        conn,
    })
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

/// Points every target's watch at its directory or nearest existing ancestor,
/// adding new watches and dropping ones no target uses. With `force`, every
/// watch is replaced, for when a watched directory was removed and recreated
/// (the kernel ended the old watch with the old inode).
fn arm(
    notifier: &mut RecommendedWatcher,
    targets: &mut [Target],
    force: bool,
) -> Result<(), ThemeError> {
    for i in 0..targets.len() {
        let want = nearest_existing(&targets[i].dir);
        let old = targets[i].watched.clone();
        if old.as_ref() == Some(&want) && !force {
            continue;
        }
        if old.as_ref() == Some(&want) {
            // Replace a watch whose directory may have been recreated.
            match notifier.unwatch(&want) {
                Ok(()) => {}
                Err(e) if matches!(e.kind, notify::ErrorKind::WatchNotFound) => {}
                Err(source) => {
                    return Err(ThemeError::Watch { path: want, source });
                }
            }
        }
        notifier
            .watch(&want, RecursiveMode::NonRecursive)
            .map_err(|source| ThemeError::Watch {
                path: want.clone(),
                source,
            })?;
        targets[i].watched = Some(want.clone());
        if let Some(old) = old.filter(|o| *o != want) {
            let still_used = targets.iter().any(|t| t.watched.as_ref() == Some(&old));
            if !still_used {
                match notifier.unwatch(&old) {
                    Ok(()) => {}
                    // The old directory is gone; the kernel ended its watch.
                    Err(e) if matches!(e.kind, notify::ErrorKind::WatchNotFound) => {}
                    Err(source) => return Err(ThemeError::Watch { path: old, source }),
                }
            }
        }
    }
    Ok(())
}

/// Whether `ev` touches a file the theme reads, or the path to a watched
/// directory that does not exist yet.
fn relevant(ev: &Event, targets: &[Target]) -> bool {
    // Opens and reads of a file change nothing; a completed write does.
    if let EventKind::Access(kind) = ev.kind {
        if !matches!(kind, AccessKind::Close(AccessMode::Write)) {
            return false;
        }
    }
    ev.paths.iter().any(|p| {
        targets.iter().any(|t| {
            // `p` is the directory or one of its ancestors (the directory
            // appeared, was removed, or its parent changed).
            t.dir.starts_with(p)
                || (p.parent() == Some(t.dir.as_path())
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| t.files.contains(&n)))
        })
    })
}

/// Whether `ev` removed or created a watched directory or one of its
/// ancestors, which can leave a watch pointing at a dead inode.
fn dir_event(ev: &Event, targets: &[Target]) -> bool {
    matches!(
        ev.kind,
        EventKind::Remove(_)
            | EventKind::Create(_)
            | EventKind::Modify(notify::event::ModifyKind::Name(_))
    ) && ev
        .paths
        .iter()
        .any(|p| targets.iter().any(|t| t.dir.starts_with(p)))
}

/// What one burst of messages amounted to.
#[derive(Default)]
struct Burst {
    /// Something the theme reads changed.
    pending: bool,
    /// A watched directory or ancestor was created, removed or renamed.
    rearm: bool,
}

/// Folds `msg` into `burst`. Returns whether it counted as a change (which
/// restarts the quiet period), or `None` for [`Msg::Stop`].
fn absorb(
    msg: Msg,
    config: &Path,
    targets: &[Target],
    errors: &mut Vec<ThemeError>,
    burst: &mut Burst,
) -> Option<bool> {
    let changed = match msg {
        Msg::Stop => return None,
        Msg::Fs(Ok(ev)) => {
            let rel = relevant(&ev, targets);
            if rel && dir_event(&ev, targets) {
                burst.rearm = true;
            }
            rel
        }
        Msg::Fs(Err(source)) => {
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

fn run(
    config: &Path,
    mut notifier: RecommendedWatcher,
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
        if absorb(first, config, &targets, &mut errors, &mut burst).is_none() {
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
                Ok(m) => match absorb(m, config, &targets, &mut errors, &mut burst) {
                    None => return,
                    Some(true) => quiet_since = Instant::now(),
                    Some(false) => {}
                },
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        if let Err(e) = arm(&mut notifier, &mut targets, burst.rearm) {
            errors.push(e);
        }
        let mut gathered = gather_from(config, conn.as_ref());
        gathered.errors.append(&mut errors);
        on_change(gathered);
    }
}
