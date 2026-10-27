// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Live volume changes from two sources.
//!
//! Specifications:
//! - proc(5), "/proc/[pid]/mountinfo": the file can be polled; after a mount
//!   or unmount in the process's namespace `poll(2)` reports `POLLERR` and
//!   `POLLPRI`, and the descriptor needs no re-reading or reopening to be
//!   polled again (poll(2) for the flags).
//! - UDisks2 D-Bus API (<https://storaged.org/doc/udisks2-api/latest/>): the
//!   `org.freedesktop.DBus.ObjectManager` signals `InterfacesAdded` and
//!   `InterfacesRemoved` at `/org/freedesktop/UDisks2`, and the `Block` and
//!   `Filesystem` interfaces. D-Bus Specification, "Standard Interfaces":
//!   `org.freedesktop.DBus.Properties.PropertiesChanged`; "Message Bus
//!   Messages": `NameHasOwner`, `ListActivatableNames`.
//!
//! Mounts and unmounts come from mountinfo alone. UDisks adds what mountinfo
//! cannot show: a device appearing or going away, a filesystem created on
//! it, a label or size change. Of UDisks' signals only those that touch the
//! `Block` and `Filesystem` interfaces and the properties [`list`] and
//! [`Udisks::unmounted`] read count; `Job` objects, drive state and the like
//! do not.
//!
//! The system bus is reached over its Unix socket through `kernkraft-bus`.
//! When it or UDisks2 is missing the mountinfo half still runs, and the
//! absence is returned by [`watch`] as a problem, once.
//!
//! Threads:
//! - the mount reader sleeps in `poll(2)` on the mountinfo descriptor and a
//!   stop eventfd;
//! - the signal reader reads UDisks' signals from the bus connection;
//! - the worker debounces both and calls back.
//!
//! Dropping a [`VolumeWatcher`] writes the eventfd, stops the worker, closes
//! the bus connection (which ends the signal reader's blocking read) and
//! waits for each thread, at most [`DROP_WAIT`] each.
//!
//! [`list`]: crate::list
//! [`Udisks::unmounted`]: crate::Udisks::unmounted

use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use kernkraft_bus::BusError;
use rustix::event::{eventfd, poll, EventfdFlags, PollFd, PollFlags};
use rustix::fs::{open, Mode, OFlags};
use rustix::io::Errno;
use zbus::blocking::fdo::DBusProxy;
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Type as MessageType;
use zbus::names::BusName;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};
use zbus::{MatchRule, Message};

use crate::udisks::{bad, BLOCK, FILESYSTEM, OBJECT_MANAGER, ROOT, SERVICE};
use crate::VolumesError;

const MOUNTINFO: &str = "/proc/self/mountinfo";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

/// Quiet time after the last change before one callback runs. Plugging in a
/// stick makes UDisks announce the drive, its block device, each partition
/// and each filesystem in turn, and a mount adds a kernel mountinfo event of
/// its own a moment later; a listing is a `GetManagedObjects` round trip.
/// Waiting for the burst to end gives one listing per burst instead of one
/// per signal. 250 ms is longer than the gaps inside such a burst seen on
/// real hardware and short enough that a list update still feels immediate.
pub const DEBOUNCE: Duration = Duration::from_millis(250);

/// A burst that never goes quiet is flushed after this long, so a source
/// that flaps continuously cannot hold the update back forever.
const MAX_DEFER: Duration = Duration::from_secs(2);

/// Longest [`VolumeWatcher`]'s drop waits for one thread. The worker is busy
/// only in the caller's callback; one that never returns would otherwise
/// hang the drop. Past this the thread is left to finish on its own: it has
/// been told to stop and exits as soon as it returns.
pub const DROP_WAIT: Duration = Duration::from_secs(10);

/// `Block` properties [`Udisks::unmounted`](crate::Udisks::unmounted) lists.
const BLOCK_PROPS: &[&str] = &[
    "Device",
    "IdLabel",
    "IdType",
    "Size",
    "HintIgnore",
    "HintSystem",
];
/// The `Filesystem` property that says whether a device is unmounted.
const FILESYSTEM_PROPS: &[&str] = &["MountPoints"];

enum Msg {
    Change,
    Error(VolumesError),
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

/// A running watch. Dropping it stops the mount watch and the UDisks
/// subscription and waits for the threads (see [`DROP_WAIT`]).
///
/// A panic in the callback is caught: the watcher keeps running and the
/// panic message is reported as [`VolumesError::CallbackPanic`] in the errors
/// of the next callback.
pub struct VolumeWatcher {
    tx: Sender<Msg>,
    stop: Arc<AtomicBool>,
    stop_fd: Arc<OwnedFd>,
    mounts: Option<Joinable>,
    worker: Option<Joinable>,
    signals: Option<Joinable>,
    conn: Option<Connection>,
}

impl Drop for VolumeWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Waking the mount reader: an eventfd takes an 8-byte counter
        // increment.
        let _ = rustix::io::write(&*self.stop_fd, &1u64.to_ne_bytes());
        // A send fails only when the worker already ended.
        let _ = self.tx.send(Msg::Stop);
        if let Some(j) = self.mounts.take() {
            j.finish();
        }
        if let Some(j) = self.worker.take() {
            j.finish();
        }
        if let Some(conn) = self.conn.take() {
            // Closing the connection ends the signal reader's blocking read.
            let _ = conn.close();
        }
        if let Some(j) = self.signals.take() {
            j.finish();
        }
    }
}

/// The callback: the errors the watch itself met since the last call (a
/// failed read, a panic in the previous callback), usually none. A call
/// means the volumes may have changed; list them again.
pub type OnChange = Box<dyn Fn(Vec<VolumesError>) + Send>;

/// Watches `/proc/self/mountinfo` and UDisks2 on the system bus, calling
/// `on_change` after each burst of changes. See [`watch_with`].
///
/// Fails only when mountinfo cannot be opened. The problems returned beside
/// the watcher are what could not be set up and so is not followed: the
/// system bus missing or unreachable, UDisks2 absent, a refused subscription.
pub fn watch(on_change: OnChange) -> Result<(VolumeWatcher, Vec<VolumesError>), VolumesError> {
    let fd = open(MOUNTINFO, OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty())
        .map_err(|e| VolumesError::io(std::path::Path::new(MOUNTINFO), e.into()))?;
    watch_with(fd, kernkraft_bus::system(), on_change)
}

/// [`watch`] on a given mountinfo descriptor and bus connection (or the
/// error opening it).
///
/// Nothing is delivered for the state at the start: start the watch, then
/// list, so a change in between is not lost. Mounts are followed through
/// `mountinfo`, which must be an open `/proc/<pid>/mountinfo`. A `system`
/// that is an error, a bus without UDisks2 (neither running nor
/// activatable), and a bus that refuses the subscription each leave out the
/// device signals and are returned as problems; the mount watch runs
/// regardless.
///
/// Callbacks run on the worker thread, [`DEBOUNCE`] after the last change.
pub fn watch_with(
    mountinfo: OwnedFd,
    system: Result<Connection, BusError>,
    on_change: OnChange,
) -> Result<(VolumeWatcher, Vec<VolumesError>), VolumesError> {
    let stop_fd =
        Arc::new(eventfd(0, EventfdFlags::CLOEXEC).map_err(|e| {
            VolumesError::io(std::path::Path::new("eventfd"), std::io::Error::from(e))
        })?);
    let (tx, rx) = mpsc::channel::<Msg>();
    let stop = Arc::new(AtomicBool::new(false));
    let mut problems = Vec::new();

    let mut conn = None;
    let mut iter = None;
    match system {
        Err(e) => problems.push(VolumesError::Bus(e)),
        Ok(c) => match subscribe(&c) {
            Ok(Some(i)) => {
                iter = Some(i);
                conn = Some(c);
            }
            Ok(None) => problems.push(VolumesError::UdisksAbsent),
            Err(e) => problems.push(e),
        },
    }

    let signals = iter.map(|iter| {
        let (stx, sstop) = (tx.clone(), stop.clone());
        spawn_joinable(move || signal_loop(iter, &sstop, &stx))
    });
    let mounts = {
        let (mtx, mfd, mstop_fd, mstop) = (tx.clone(), mountinfo, stop_fd.clone(), stop.clone());
        spawn_joinable(move || mount_loop(&mfd, &mstop_fd, &mstop, &mtx))
    };
    let worker = spawn_joinable(move || run(rx, on_change));

    Ok((
        VolumeWatcher {
            tx,
            stop,
            stop_fd,
            mounts: Some(mounts),
            worker: Some(worker),
            signals,
            conn,
        },
        problems,
    ))
}

/// Whether UDisks2 is on the bus now (it owns its name) or can be started
/// there (it is activatable).
fn udisks_on_bus(owned: bool, activatable: &[String]) -> bool {
    owned || activatable.iter().any(|n| n == SERVICE)
}

/// Subscribes to UDisks2's signals. `None` when UDisks2 is not on the bus.
fn subscribe(conn: &Connection) -> Result<Option<MessageIterator>, VolumesError> {
    let what = "bus name lookup";
    let dbus = DBusProxy::new(conn).map_err(|e| bad(what, e.to_string()))?;
    let name = BusName::try_from(SERVICE).map_err(|e| bad(what, e.to_string()))?;
    let owned = dbus
        .name_has_owner(name)
        .map_err(|e| bad("NameHasOwner", e.to_string()))?;
    let activatable: Vec<String> = dbus
        .list_activatable_names()
        .map_err(|e| bad("ListActivatableNames", e.to_string()))?
        .into_iter()
        .map(|n| n.to_string())
        .collect();
    if !udisks_on_bus(owned, &activatable) {
        return Ok(None);
    }
    // The match goes to the bus daemon, not to UDisks, so it holds across
    // UDisks starting and stopping. Every UDisks signal arrives and
    // `changes_devices` picks the ones that matter: one rule and one reader
    // instead of one per interface.
    let rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(SERVICE)
        .map(|b| b.build())
        .map_err(|e| bad("match rule", e.to_string()))?;
    MessageIterator::for_match_rule(rule, conn, None)
        .map(Some)
        .map_err(|e| bad("subscribe", e.to_string()))
}

/// Sleeps until the mountinfo descriptor reports a change or the stop
/// eventfd is readable. A change is `POLLERR | POLLPRI` (proc(5)); any other
/// flag means the descriptor is not what this expects, which is reported
/// and ends the loop rather than spinning on it.
fn mount_loop(mountinfo: &OwnedFd, stop_fd: &OwnedFd, stop: &AtomicBool, tx: &Sender<Msg>) {
    loop {
        let mut fds = [
            PollFd::new(mountinfo, PollFlags::PRI),
            PollFd::new(stop_fd, PollFlags::IN),
        ];
        match poll(&mut fds, None) {
            Ok(_) => {}
            Err(Errno::INTR) => continue,
            Err(e) => {
                let _ = tx.send(Msg::Error(VolumesError::io(
                    std::path::Path::new(MOUNTINFO),
                    e.into(),
                )));
                return;
            }
        }
        if stop.load(Ordering::SeqCst) || !fds[1].revents().is_empty() {
            return;
        }
        let ev = fds[0].revents();
        if ev.is_empty() {
            continue;
        }
        if !(ev - (PollFlags::PRI | PollFlags::ERR)).is_empty() {
            let _ = tx.send(Msg::Error(bad(
                "mountinfo poll",
                format!("unexpected poll flags {ev:?}"),
            )));
            return;
        }
        if tx.send(Msg::Change).is_err() {
            return;
        }
    }
}

/// Whether `msg` is a UDisks signal that changes what [`list`](crate::list)
/// and [`Udisks::unmounted`](crate::Udisks::unmounted) return. A signal
/// whose body does not have the documented shape is an error, not
/// ignored.
fn changes_devices(msg: &Message) -> Result<bool, VolumesError> {
    let header = msg.header();
    let (Some(iface), Some(member)) = (header.interface(), header.member()) else {
        return Ok(false);
    };
    let body = msg.body();
    let relevant = |name: &str| name == BLOCK || name == FILESYSTEM;
    match (iface.as_str(), member.as_str()) {
        (OBJECT_MANAGER, "InterfacesAdded")
            if header.path().is_some_and(|p| p.as_str() == ROOT) =>
        {
            let (_, ifaces) = body
                .deserialize::<(
                    OwnedObjectPath,
                    HashMap<String, HashMap<String, OwnedValue>>,
                )>()
                .map_err(|e| bad("InterfacesAdded", e.to_string()))?;
            Ok(ifaces.keys().any(|k| relevant(k)))
        }
        (OBJECT_MANAGER, "InterfacesRemoved")
            if header.path().is_some_and(|p| p.as_str() == ROOT) =>
        {
            let (_, ifaces) = body
                .deserialize::<(OwnedObjectPath, Vec<String>)>()
                .map_err(|e| bad("InterfacesRemoved", e.to_string()))?;
            Ok(ifaces.iter().any(|k| relevant(k)))
        }
        (PROPERTIES, "PropertiesChanged") => {
            let (name, changed, invalidated) = body
                .deserialize::<(String, HashMap<String, OwnedValue>, Vec<String>)>()
                .map_err(|e| bad("PropertiesChanged", e.to_string()))?;
            let props = match name.as_str() {
                BLOCK => BLOCK_PROPS,
                FILESYSTEM => FILESYSTEM_PROPS,
                _ => return Ok(false),
            };
            Ok(changed
                .keys()
                .chain(invalidated.iter())
                .any(|k| props.contains(&k.as_str())))
        }
        _ => Ok(false),
    }
}

/// Reads UDisks' signals until the iterator ends (the connection was
/// closed) or fails.
fn signal_loop<I>(iter: I, stop: &AtomicBool, tx: &Sender<Msg>)
where
    I: IntoIterator<Item = zbus::Result<Message>>,
{
    for msg in iter {
        let sent = match msg {
            Ok(m) => match changes_devices(&m) {
                Ok(true) => tx.send(Msg::Change),
                Ok(false) => Ok(()),
                Err(e) => tx.send(Msg::Error(e)),
            },
            Err(e) => {
                if !stop.load(Ordering::SeqCst) {
                    let _ = tx.send(Msg::Error(bad("signals", e.to_string())));
                }
                return;
            }
        };
        if sent.is_err() {
            return;
        }
    }
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

/// Folds `msg` into the pending errors. `None` for [`Msg::Stop`].
fn absorb(msg: Msg, errors: &mut Vec<VolumesError>) -> Option<()> {
    match msg {
        Msg::Stop => return None,
        Msg::Change => {}
        // Delivered even when no change came with it: a source that
        // stopped working is news.
        Msg::Error(e) => errors.push(e),
    }
    Some(())
}

fn run(rx: mpsc::Receiver<Msg>, on_change: OnChange) {
    let mut errors: Vec<VolumesError> = Vec::new();
    loop {
        let Ok(first) = rx.recv() else { return };
        if absorb(first, &mut errors).is_none() {
            return;
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
                Ok(m) => {
                    if absorb(m, &mut errors).is_none() {
                        return;
                    }
                    quiet_since = Instant::now();
                }
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        let batch = std::mem::take(&mut errors);
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| on_change(batch))) {
            errors.push(VolumesError::CallbackPanic(panic_message(&*payload)));
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::sync::Mutex;

    use zbus::zvariant::{ObjectPath, Value};

    use super::*;

    fn signal<B>(path: &str, iface: &str, member: &str, body: &B) -> Message
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
    {
        Message::signal(path, iface, member)
            .unwrap()
            .build(body)
            .unwrap()
    }

    fn added(path: &str, ifaces: &[&str]) -> Message {
        let body: HashMap<String, HashMap<String, Value<'_>>> = ifaces
            .iter()
            .map(|i| ((*i).to_string(), HashMap::new()))
            .collect();
        signal(
            ROOT,
            OBJECT_MANAGER,
            "InterfacesAdded",
            &(ObjectPath::try_from(path).unwrap(), body),
        )
    }

    fn removed(path: &str, ifaces: &[&str]) -> Message {
        let body: Vec<String> = ifaces.iter().map(|s| (*s).to_string()).collect();
        signal(
            ROOT,
            OBJECT_MANAGER,
            "InterfacesRemoved",
            &(ObjectPath::try_from(path).unwrap(), body),
        )
    }

    fn props_changed(iface: &str, changed: &[&str], invalidated: &[&str]) -> Message {
        let changed: HashMap<String, Value<'_>> = changed
            .iter()
            .map(|k| ((*k).to_string(), Value::U64(1)))
            .collect();
        let invalidated: Vec<String> = invalidated.iter().map(|s| (*s).to_string()).collect();
        signal(
            "/org/freedesktop/UDisks2/block_devices/sdb1",
            PROPERTIES,
            "PropertiesChanged",
            &(iface.to_string(), changed, invalidated),
        )
    }

    const SDB1: &str = "/org/freedesktop/UDisks2/block_devices/sdb1";
    const JOB: &str = "org.freedesktop.UDisks2.Job";

    #[test]
    fn added_block_or_filesystem_counts() {
        assert!(changes_devices(&added(SDB1, &[BLOCK])).unwrap());
        assert!(changes_devices(&added(SDB1, &[FILESYSTEM, "x.y.Z"])).unwrap());
    }

    #[test]
    fn added_job_or_drive_alone_does_not() {
        let job = "/org/freedesktop/UDisks2/jobs/1";
        assert!(!changes_devices(&added(job, &[JOB])).unwrap());
        let drive = "/org/freedesktop/UDisks2/drives/x";
        assert!(!changes_devices(&added(drive, &["org.freedesktop.UDisks2.Drive"])).unwrap());
    }

    #[test]
    fn removed_block_or_filesystem_counts() {
        assert!(changes_devices(&removed(SDB1, &[BLOCK, FILESYSTEM])).unwrap());
        assert!(changes_devices(&removed(SDB1, &[FILESYSTEM])).unwrap());
        assert!(!changes_devices(&removed("/org/freedesktop/UDisks2/jobs/1", &[JOB])).unwrap());
    }

    #[test]
    fn properties_changed_only_for_the_listed_properties() {
        assert!(changes_devices(&props_changed(BLOCK, &["IdLabel"], &[])).unwrap());
        assert!(changes_devices(&props_changed(BLOCK, &[], &["IdType"])).unwrap());
        assert!(changes_devices(&props_changed(FILESYSTEM, &["MountPoints"], &[])).unwrap());
        // Properties no listing reads.
        assert!(!changes_devices(&props_changed(BLOCK, &["Configuration"], &[])).unwrap());
        // An interface no listing reads.
        assert!(!changes_devices(&props_changed(JOB, &["Progress", "IdLabel"], &[])).unwrap());
    }

    #[test]
    fn other_signals_do_not() {
        let m = signal(ROOT, "org.freedesktop.UDisks2.Manager", "Anything", &());
        assert!(!changes_devices(&m).unwrap());
        // InterfacesAdded from another object is not the ObjectManager of
        // UDisks2.
        let body: HashMap<String, HashMap<String, Value<'_>>> =
            HashMap::from([(BLOCK.to_string(), HashMap::new())]);
        let m = signal(
            "/elsewhere",
            OBJECT_MANAGER,
            "InterfacesAdded",
            &(ObjectPath::try_from(SDB1).unwrap(), body),
        );
        assert!(!changes_devices(&m).unwrap());
    }

    #[test]
    fn malformed_bodies_are_errors() {
        for (member, iface) in [
            ("InterfacesAdded", OBJECT_MANAGER),
            ("InterfacesRemoved", OBJECT_MANAGER),
            ("PropertiesChanged", PROPERTIES),
        ] {
            let m = signal(ROOT, iface, member, &42u32);
            match changes_devices(&m) {
                Err(VolumesError::Dbus { what, .. }) => assert_eq!(what, member),
                other => panic!("{member}: {other:?}"),
            }
        }
    }

    fn collect() -> (OnChange, mpsc::Receiver<(Instant, Vec<VolumesError>)>) {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        (
            Box::new(move |e| {
                let _ = tx.lock().unwrap().send((Instant::now(), e));
            }),
            rx,
        )
    }

    /// The signal reader turns constructed messages into changes and errors
    /// and ends when its iterator does.
    #[test]
    fn signal_loop_forwards_changes_and_errors() {
        let (tx, rx) = mpsc::channel();
        let items: Vec<zbus::Result<Message>> = vec![
            Ok(added("/org/freedesktop/UDisks2/jobs/1", &[JOB])),
            Ok(added(SDB1, &[BLOCK])),
            Ok(signal(ROOT, OBJECT_MANAGER, "InterfacesRemoved", &42u32)),
            Err(zbus::Error::Failure("boom".into())),
            Ok(added(SDB1, &[BLOCK])),
        ];
        signal_loop(items, &AtomicBool::new(false), &tx);
        drop(tx);
        let got: Vec<Msg> = rx.iter().collect();
        // The job is ignored; the block counts; the malformed body is an
        // error; the transport error is an error and ends the loop.
        assert!(matches!(got[0], Msg::Change));
        assert!(
            matches!(&got[1], Msg::Error(VolumesError::Dbus { what, .. }) if what == "InterfacesRemoved")
        );
        assert!(
            matches!(&got[2], Msg::Error(VolumesError::Dbus { what, .. }) if what == "signals")
        );
        assert_eq!(got.len(), 3);
    }

    #[test]
    fn signal_loop_after_stop_is_quiet() {
        let (tx, rx) = mpsc::channel();
        let items: Vec<zbus::Result<Message>> = vec![Err(zbus::Error::Failure("closed".into()))];
        signal_loop(items, &AtomicBool::new(true), &tx);
        drop(tx);
        assert_eq!(rx.iter().count(), 0);
    }

    #[test]
    fn udisks_presence() {
        assert!(udisks_on_bus(true, &[]));
        assert!(udisks_on_bus(false, &["a.b".into(), SERVICE.into()]));
        assert!(!udisks_on_bus(false, &["a.b".into()]));
    }

    /// A burst of changes inside the quiet time is one callback.
    #[test]
    fn burst_is_one_callback() {
        let (tx, rx) = mpsc::channel();
        let (cb, got) = collect();
        let j = spawn_joinable(move || run(rx, cb));
        let mut last = Instant::now();
        for _ in 0..10 {
            tx.send(Msg::Change).unwrap();
            last = Instant::now();
            thread::sleep(DEBOUNCE / 10);
        }
        let (at, errors) = got.recv_timeout(DEBOUNCE * 10).unwrap();
        assert!(errors.is_empty());
        // Fired after the last change plus the quiet time, not before, and
        // once.
        assert!(at - last >= DEBOUNCE);
        assert!(got.recv_timeout(DEBOUNCE * 2).is_err(), "a second callback");
        tx.send(Msg::Stop).unwrap();
        j.finish();
    }

    /// Changes further apart than the quiet time are separate callbacks, and
    /// the callback waits the quiet time after the change.
    #[test]
    fn separated_changes_are_separate_callbacks() {
        let (tx, rx) = mpsc::channel();
        let (cb, got) = collect();
        let j = spawn_joinable(move || run(rx, cb));
        let t0 = Instant::now();
        tx.send(Msg::Change).unwrap();
        let (first, _) = got.recv_timeout(DEBOUNCE * 10).unwrap();
        assert!(first - t0 >= DEBOUNCE);
        tx.send(Msg::Change).unwrap();
        got.recv_timeout(DEBOUNCE * 10).unwrap();
        tx.send(Msg::Stop).unwrap();
        j.finish();
    }

    /// A source that never goes quiet is flushed after MAX_DEFER.
    #[test]
    fn endless_burst_is_flushed() {
        let (tx, rx) = mpsc::channel();
        let (cb, got) = collect();
        let j = spawn_joinable(move || run(rx, cb));
        let t0 = Instant::now();
        let feeder = {
            let tx = tx.clone();
            thread::spawn(move || {
                while t0.elapsed() < MAX_DEFER + Duration::from_secs(1) {
                    if tx.send(Msg::Change).is_err() {
                        return;
                    }
                    thread::sleep(DEBOUNCE / 5);
                }
            })
        };
        let (at, _) = got
            .recv_timeout(MAX_DEFER + Duration::from_secs(1))
            .unwrap();
        // Quiet time never elapsed, so this is the cap.
        assert!(at - t0 >= MAX_DEFER);
        assert!(at - t0 < MAX_DEFER + DEBOUNCE * 4);
        feeder.join().unwrap();
        tx.send(Msg::Stop).unwrap();
        j.finish();
    }

    /// An error with no change still produces a callback that carries it.
    #[test]
    fn error_alone_is_delivered() {
        let (tx, rx) = mpsc::channel();
        let (cb, got) = collect();
        let j = spawn_joinable(move || run(rx, cb));
        tx.send(Msg::Error(VolumesError::UdisksAbsent)).unwrap();
        let (_, errors) = got.recv_timeout(DEBOUNCE * 10).unwrap();
        assert!(matches!(errors[..], [VolumesError::UdisksAbsent]));
        tx.send(Msg::Stop).unwrap();
        j.finish();
    }

    /// A panicking callback is reported with the next callback and the
    /// worker survives.
    #[test]
    fn callback_panic_is_reported_next_time() {
        let (tx, rx) = mpsc::channel();
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let (rtx, got) = mpsc::channel();
        let rtx = Mutex::new(rtx);
        let cb: OnChange = Box::new(move |errors| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                panic!("first callback fails");
            }
            let _ = rtx.lock().unwrap().send(errors);
        });
        let j = spawn_joinable(move || run(rx, cb));
        tx.send(Msg::Change).unwrap();
        thread::sleep(DEBOUNCE * 3);
        tx.send(Msg::Change).unwrap();
        let errors = got.recv_timeout(DEBOUNCE * 10).unwrap();
        match &errors[..] {
            [VolumesError::CallbackPanic(m)] => assert_eq!(m, "first callback fails"),
            other => panic!("{other:?}"),
        }
        tx.send(Msg::Stop).unwrap();
        j.finish();
    }
}
