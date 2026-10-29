// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Volumes for the Places panel: what is mounted (proc(5),
//! `/proc/self/mountinfo`), what could be (UDisks2 D-Bus API), and mount,
//! unmount and eject through UDisks2.
//!
//! Mount, unmount and eject are plain commands, run on Tauri's blocking
//! pool like every other, whose promise settles when UDisks answers; they
//! are not background operations in the registry. They can take long (a
//! polkit prompt waits on the user, up to `kernkraft_volumes::ACTION_TIMEOUT`),
//! but there is nothing to report on the way and nothing to cancel: UDisks
//! offers no way to cancel a call in flight, and the user ends a prompt by
//! dismissing it, which the command reports as `NotAuthorized`. Nor do they
//! touch files the undo journal knows, so they have no reason to make undo
//! wait. The UI shows the volume as busy until the promise settles.
//!
//! The list updates itself: the first [`Kernkraft::volumes_list`] starts a
//! watcher (`kernkraft_volumes::watch`: mounts and unmounts from
//! `/proc/self/mountinfo`, devices appearing and going away from UDisks2's
//! signals) and every later change is sent as [`EVENT_VOLUMES_CHANGED`]. The
//! watcher starts then and not at plugin setup, for the reason the theme
//! watcher does: until the app asks for volumes nobody needs the updates,
//! and starting it opens the system bus. It starts before the first listing,
//! so a change between the two is not lost; the event means "list again".
//!
//! Events are debounced by `kernkraft_volumes::DEBOUNCE` (250 ms of quiet
//! after the last change, at most 2 s from the first): plugging in a stick
//! is a burst of UDisks signals and a mount event, and each listing is a
//! round trip to UDisks, so a burst becomes one event and one listing.
//!
//! What the watcher could not set up (the system bus or UDisks2 missing, a
//! refused subscription) is in the `problems` of the first
//! [`Kernkraft::volumes_list`] only; the mount watch runs regardless. A
//! failure to open mountinfo leaves no watcher, is reported in that call's
//! `problems`, and is tried again by the next call. Errors the watcher meets
//! later, and a panic in the event callback, are in the `problems` of the
//! next event.
//!
//! These once-only problems (the startup ones and the runtime ones) are not
//! lost with the webview that was listening: they are also held here until
//! [`Kernkraft::volumes_problems_take`] (`volumes_problems_take`) returns
//! them and clears them. A consumer that reloads takes them on load, before
//! it lists, and keeps them on screen for as long as they apply. The
//! contract:
//!
//! - A problem is held from the moment it is found, whether or not any
//!   window was listening, and stays held until a take. The first listing
//!   still reports the startup problems in its `problems` and an event still
//!   carries the runtime ones, so a problem can reach a consumer by both
//!   routes; a problem is identified by its `kind`, `message` and `path`,
//!   and a consumer shows each once.
//! - A take returns every held problem, oldest first, and clears them. It
//!   also settles the startup problems no listing has reported yet, so the
//!   next listing does not repeat what the consumer already has. Windows
//!   share the hold: whichever takes first gets them.
//! - A problem identical to one already held is held once, so UDisks2
//!   leaving and returning over and over does not grow the hold. At most
//!   [`MAX_HELD_PROBLEMS`] distinct problems are held, the first ones; the
//!   take then ends with one more problem, `ProblemsNotShown`, saying how many
//!   more arrived and were not kept ("N more not shown").
//! - Not held, because they are not once-only: what the listing itself
//!   finds (`problems` of every listing) and a watcher that failed to start
//!   (reported by each listing until it does).
//!
//! A consumer listens for the event before its first `volumes_list`, and
//! keeps what the first listing reported on screen for as long as it applies:
//! those startup problems come back once and never again, while later events
//! carry only what happened since the previous one.

use std::path::PathBuf;
use std::sync::Arc;

use kernkraft_volumes::{
    find_listed, list, verify_mount, watch, Device, DiskId, Identity, ListProblem, Mount, OnChange,
    Udisks, VolumeWatcher, VolumesError,
};
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Runtime};

use crate::error::{PluginError, WireError};
use crate::state::{lock, Kernkraft};
use crate::wire::WirePath;

pub const EVENT_VOLUMES_CHANGED: &str = "kernkraft://volumes-changed";

/// Most once-only problems held for a take. A healthy run produces a few (no
/// system bus, UDisks2 leaving and returning), and a consumer takes them on
/// every load; the bound is for one that never takes, and sits far above
/// what a run produces. Problems beyond it are counted and reported by the
/// take (`PluginError::ProblemsNotShown`), not dropped silently.
pub(crate) const MAX_HELD_PROBLEMS: usize = 64;

/// The payload of [`EVENT_VOLUMES_CHANGED`]: the volumes may have changed,
/// list them again. `problems` are errors of the watch itself since the last
/// event, usually none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumesChanged {
    pub problems: Vec<WireError>,
}

#[derive(Default)]
pub(crate) struct VolumesState {
    started: bool,
    /// What starting the watcher could not follow, reported by the next
    /// listing only.
    once: Vec<WireError>,
    /// Every once-only problem not yet taken, without repeats, at most
    /// [`MAX_HELD_PROBLEMS`] (see the module documentation).
    held: Vec<WireError>,
    /// Problems that arrived while `held` was full.
    not_kept: usize,
    watcher: Option<VolumeWatcher>,
}

impl VolumesState {
    /// Holds `problems` for a take, leaving out any already held. Those that
    /// do not fit are counted.
    fn hold(&mut self, problems: &[WireError]) {
        for problem in problems {
            if self.held.contains(problem) {
                continue;
            }
            if self.held.len() < MAX_HELD_PROBLEMS {
                self.held.push(problem.clone());
            } else {
                self.not_kept += 1;
            }
        }
    }

    /// The held problems, oldest first, followed by one that says how many
    /// did not fit, if any. Empties both.
    fn take_held(&mut self) -> Vec<WireError> {
        let mut taken = std::mem::take(&mut self.held);
        let count = std::mem::take(&mut self.not_kept);
        if count > 0 {
            taken.push(WireError::from(PluginError::ProblemsNotShown {
                count,
                kept: MAX_HELD_PROBLEMS,
            }));
        }
        taken
    }
}

/// What starts a watcher: `kernkraft_volumes::watch`, or a stand-in.
type WatchFn<'a> =
    dyn Fn(OnChange) -> Result<(VolumeWatcher, Vec<VolumesError>), VolumesError> + 'a;

/// A mounted volume. `mount_point` is the exact path; the rest are display
/// strings (see `kernkraft_volumes::Mount`). `identity` says which disk it is
/// on: `{state: "known", major, minor, diskseq}` to pass back as `disk` to
/// `volume_unmount` and `volume_eject`, which refuse with `IdentityMismatch`
/// when the disk was swapped since the listing, or `{state: "unknown",
/// reason}` for a volume that has none (those two commands cannot be used on
/// it). `major:minor` is unique among the block devices present now and
/// reused at once when one is unplugged; `diskseq` is unique per disk object
/// until reboot and shared by a disk's partitions. A stick reformatted in
/// place, or a card swapped in a reader that reports no media change, keeps
/// both.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireMount {
    pub mount_point: WirePath,
    pub identity: Identity,
    pub device: String,
    pub fs_type: String,
    pub label: String,
    pub removable: bool,
    pub read_only: bool,
}

impl From<&Mount> for WireMount {
    fn from(m: &Mount) -> WireMount {
        WireMount {
            mount_point: WirePath::from(&m.mount_point),
            identity: m.identity.clone(),
            device: m.device.clone(),
            fs_type: m.fs_type.clone(),
            label: m.label.clone(),
            removable: m.removable,
            read_only: m.read_only,
        }
    }
}

/// The Places panel's volumes: mounted ones, filesystems UDisks could
/// mount, and what could not be found out. A problem with one source
/// leaves the other's volumes listed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireVolumes {
    pub mounted: Vec<WireMount>,
    pub unmounted: Vec<Device>,
    pub problems: Vec<WireError>,
}

/// A listing problem as a [`WireError`] whose `kind` is the
/// `ListProblem` variant's name.
fn problem(p: &ListProblem) -> WireError {
    let (kind, message) = match p {
        ListProblem::MalformedLine { line, reason } => (
            "MalformedLine",
            format!("mountinfo line {line} skipped, so a volume may be missing: {reason}"),
        ),
        ListProblem::Labels(e) => (
            "Labels",
            format!("volume labels could not be read; folder names are shown: {e}"),
        ),
        ListProblem::Passwd(e) => ("Passwd", format!("/etc/passwd could not be read: {e}")),
        ListProblem::NoUserName { uid } => (
            "NoUserName",
            format!("no user name for uid {uid}, so volumes under /run/media are missing"),
        ),
    };
    WireError {
        kind: kind.to_owned(),
        message,
        path: None,
        details: None,
    }
}

impl<R: Runtime> Kernkraft<R> {
    /// The mounted volumes, and the filesystems UDisks lists as unmounted.
    /// Only failing to read `/proc/self/mountinfo` fails the call; UDisks or
    /// the system bus being unavailable is a problem in the listing, with
    /// `unmounted` empty.
    pub fn volumes_list(&self) -> Result<WireVolumes, WireError> {
        self.volumes_list_with(&watch)
    }

    /// [`Kernkraft::volumes_list`] starting its watcher through `start`.
    fn volumes_list_with(&self, start: &WatchFn<'_>) -> Result<WireVolumes, WireError> {
        let mut once = self.volumes_start(start);
        let listing = match list() {
            Ok(l) => l,
            Err(e) => {
                // Not reported yet: the next listing carries them.
                lock(&self.0.volumes).once.splice(0..0, once);
                return Err(e.into());
            }
        };
        let mut problems: Vec<WireError> = std::mem::take(&mut once);
        problems.extend(listing.problems.iter().map(problem));
        let unmounted = match Udisks::connect().and_then(|u| u.unmounted()) {
            Ok(devices) => devices,
            Err(e) => {
                problems.push(WireError::from(e));
                Vec::new()
            }
        };
        Ok(WireVolumes {
            mounted: listing.mounts.iter().map(WireMount::from).collect(),
            unmounted,
            problems,
        })
    }

    /// Starts the watcher, once, and returns the problems of starting it
    /// that have not been reported yet.
    fn volumes_start(&self, start: &WatchFn<'_>) -> Vec<WireError> {
        let mut state = lock(&self.0.volumes);
        if !state.started {
            let weak = Arc::downgrade(&self.0);
            let on_change: OnChange = Box::new(move |errors| {
                if let Some(inner) = weak.upgrade() {
                    let problems: Vec<WireError> = errors.iter().map(WireError::from).collect();
                    // Held before it is sent: a consumer that takes on the
                    // event finds them.
                    lock(&inner.volumes).hold(&problems);
                    let _ = inner
                        .app
                        .emit(EVENT_VOLUMES_CHANGED, VolumesChanged { problems });
                }
            });
            match start(on_change) {
                Ok((watcher, problems)) => {
                    state.started = true;
                    state.watcher = Some(watcher);
                    let problems: Vec<WireError> = problems.iter().map(WireError::from).collect();
                    state.hold(&problems);
                    state.once.extend(problems);
                }
                // No watcher; the next listing tries again, and this one
                // says why there is none.
                Err(e) => return vec![WireError::from(e)],
            }
        }
        std::mem::take(&mut state.once)
    }

    /// The once-only problems held since the last take, oldest first, and
    /// clears them: what starting the watcher could not follow, errors the
    /// watcher met later, a panic in the event callback. Also settles the
    /// startup problems no listing has reported yet. See the module
    /// documentation for the contract.
    pub fn volumes_problems_take(&self) -> Vec<WireError> {
        let mut state = lock(&self.0.volumes);
        state.once.clear();
        state.take_held()
    }

    /// Stops the watcher. Dropped outside the lock: the drop waits for the
    /// watcher's worker, which may be inside a callback.
    pub(crate) fn volumes_shutdown(&self) {
        let watcher = lock(&self.0.volumes).watcher.take();
        drop(watcher);
    }

    /// Mounts `device`, which must be one UDisks lists as unmounted now
    /// (the webview's copy is not trusted), and returns where UDisks
    /// mounted it. `device.identity` must be known (`IdentityUnknown`
    /// otherwise). The device is matched by object path and must be the same
    /// disk there (device number and disk sequence number, read again now):
    /// a different disk at that path is `IdentityMismatch`; the same disk
    /// under another path is found by its identity. `UnknownDevice` when it
    /// is not listed.
    pub fn volume_mount(&self, device: Device) -> Result<WirePath, WireError> {
        let wanted = device.identity.known()?;
        let udisks = Udisks::connect()?;
        let listed = find_listed(udisks.unmounted()?, &device.object_path, wanted)?
            .ok_or_else(|| PluginError::UnknownDevice(device.object_path.clone()))?;
        Ok(WirePath::from(&udisks.mount(&listed)?))
    }

    /// Unmounts the filesystem at `mount_point`, which must still be on the
    /// disk `disk` names (the identity it was listed with): the mount must be
    /// there on that block device and the disk's sequence number is read
    /// again now. `IdentityMismatch` if not, `NoDiskSeq` if the sequence
    /// number cannot be read; nothing is unmounted. The check and UDisks'
    /// action are two steps; a swap between them is not caught. `Busy` (with
    /// UDisks' message) while something uses it.
    pub fn volume_unmount(&self, mount_point: WirePath, disk: DiskId) -> Result<(), WireError> {
        let mount_point = PathBuf::try_from(mount_point)?;
        verify_mount(&mount_point, &disk)?;
        Ok(Udisks::connect()?.unmount(&mount_point)?)
    }

    /// Unmounts every volume on the drive of the volume at `mount_point` and
    /// ejects or powers off the drive (see `kernkraft_volumes::Udisks::eject`
    /// for what is refused), after the same identity check as
    /// [`Kernkraft::volume_unmount`]. A failure after something was unmounted
    /// is `EjectIncomplete`, whose details list what was.
    pub fn volume_eject(&self, mount_point: WirePath, disk: DiskId) -> Result<(), WireError> {
        let mount_point = PathBuf::try_from(mount_point)?;
        verify_mount(&mount_point, &disk)?;
        Ok(Udisks::connect()?.eject(&mount_point)?)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::cell::Cell;
    use std::fs::File;
    use std::os::fd::OwnedFd;
    use std::sync::mpsc;
    use std::sync::Mutex;
    use std::time::Duration;

    use kernkraft_bus::BusError;
    use kernkraft_volumes::{watch_with, DiskId};
    use tauri::test::{mock_app, MockRuntime};
    use tauri::Listener;

    use super::*;
    use crate::state::Config;

    fn kk() -> (tauri::App<MockRuntime>, Kernkraft<MockRuntime>) {
        let app = mock_app();
        let kk = Kernkraft::new(app.handle().clone(), Config::default());
        (app, kk)
    }

    /// A watcher on this process's mountinfo with no bus, as
    /// `kernkraft_volumes::watch` returns on a machine without one, and the
    /// callback it was given kept for the test to call. `problems` is what
    /// starting it reports.
    struct Stand {
        calls: Cell<u32>,
        cb: Mutex<Option<OnChange>>,
        problems: fn() -> Vec<VolumesError>,
    }

    impl Stand {
        fn new(problems: fn() -> Vec<VolumesError>) -> Stand {
            Stand {
                calls: Cell::new(0),
                cb: Mutex::new(None),
                problems,
            }
        }

        fn start(&self, cb: OnChange) -> Result<(VolumeWatcher, Vec<VolumesError>), VolumesError> {
            self.calls.set(self.calls.get() + 1);
            *self.cb.lock().unwrap() = Some(cb);
            let fd = OwnedFd::from(File::open("/proc/self/mountinfo").unwrap());
            let absent = BusError::NoAddress("DBUS_SYSTEM_BUS_ADDRESS");
            let (w, _) = watch_with(fd, Err(absent), Box::new(|_| {}))?;
            Ok((w, (self.problems)()))
        }

        fn fire(&self, errors: Vec<VolumesError>) {
            (self.cb.lock().unwrap().as_ref().expect("started"))(errors);
        }
    }

    fn kinds(v: &WireVolumes) -> Vec<String> {
        v.problems.iter().map(|p| p.kind.clone()).collect()
    }

    /// Nothing is watched until the first listing; the first listing starts
    /// one watcher and reports what it could not follow; later listings
    /// start nothing and do not repeat it.
    #[test]
    fn first_listing_starts_the_watcher_once() {
        let (_app, kk) = kk();
        let stand = Stand::new(|| vec![VolumesError::UdisksAbsent]);
        assert!(lock(&kk.0.volumes).watcher.is_none());
        let start = |cb| stand.start(cb);

        let first = kk.volumes_list_with(&start).unwrap();
        assert_eq!(stand.calls.get(), 1);
        assert!(lock(&kk.0.volumes).watcher.is_some());
        assert_eq!(
            kinds(&first)
                .iter()
                .filter(|k| *k == "UdisksAbsent")
                .count(),
            1
        );

        let second = kk.volumes_list_with(&start).unwrap();
        assert_eq!(stand.calls.get(), 1);
        assert!(!kinds(&second).iter().any(|k| k == "UdisksAbsent"));
    }

    /// A watcher that fails to start is reported by that listing and tried
    /// again by the next.
    #[test]
    fn failed_start_is_reported_and_retried() {
        let (_app, kk) = kk();
        let calls = Cell::new(0);
        let start = |_cb| {
            calls.set(calls.get() + 1);
            Err(VolumesError::Io {
                path: PathBuf::from("/proc/self/mountinfo"),
                source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            })
        };
        for n in 1..=2 {
            let v = kk.volumes_list_with(&start).unwrap();
            assert_eq!(calls.get(), n);
            assert!(kinds(&v).iter().any(|k| k == "Io"), "{:?}", v.problems);
            assert!(lock(&kk.0.volumes).watcher.is_none());
        }
    }

    /// The watcher's callback becomes the event, with its errors as
    /// problems.
    #[test]
    fn callback_emits_the_event() {
        let (app, kk) = kk();
        let (tx, rx) = mpsc::channel();
        app.listen(EVENT_VOLUMES_CHANGED, move |e| {
            let _ = tx.send(e.payload().to_owned());
        });
        let stand = Stand::new(Vec::new);
        kk.volumes_list_with(&|cb| stand.start(cb)).unwrap();

        stand.fire(Vec::new());
        let p: VolumesChanged =
            serde_json::from_str(&rx.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap();
        assert!(p.problems.is_empty());

        stand.fire(vec![VolumesError::CallbackPanic("x".into())]);
        let p: VolumesChanged =
            serde_json::from_str(&rx.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap();
        assert_eq!(p.problems.len(), 1);
        assert_eq!(p.problems[0].kind, "CallbackPanic");
    }

    /// A listed mount carries its identity, known or with the reason it has
    /// none.
    #[test]
    fn wire_mount_carries_its_identity() {
        let mut m = Mount {
            major: 8,
            minor: 17,
            identity: Identity::Known(DiskId {
                major: 8,
                minor: 17,
                diskseq: 9,
            }),
            mount_point: PathBuf::from("/run/media/u/S"),
            device: "/dev/sdb1".into(),
            fs_type: "exfat".into(),
            label: "S".into(),
            removable: true,
            read_only: false,
        };
        assert_eq!(WireMount::from(&m).identity, m.identity);
        m.identity = Identity::Unknown {
            reason: "no diskseq".into(),
        };
        let json = serde_json::to_value(WireMount::from(&m)).unwrap();
        assert_eq!(json["identity"]["state"], "unknown");
        assert_eq!(json["identity"]["reason"], "no diskseq");
    }

    /// Startup problems that arrive while earlier ones are unreported are
    /// added to them, not substituted.
    #[test]
    fn unreported_startup_problems_accumulate() {
        let (_app, kk) = kk();
        lock(&kk.0.volumes).once = vec![WireError::from(VolumesError::UdisksAbsent)];
        let stand = Stand::new(|| vec![VolumesError::CallbackPanic("p".into())]);
        let once = kk.volumes_start(&|cb| stand.start(cb));
        let kinds: Vec<_> = once.iter().map(|p| p.kind.as_str()).collect();
        assert_eq!(kinds, ["UdisksAbsent", "CallbackPanic"]);
    }

    /// A hold that is full keeps what it has and counts the rest, and a take
    /// reports the count as a problem of its own, so nothing is dropped
    /// without a word. A problem already held is not counted.
    #[test]
    fn held_problems_are_capped_and_the_overflow_is_reported() {
        let (_app, kk) = kk();
        let stand = Stand::new(Vec::new);
        kk.volumes_list_with(&|cb| stand.start(cb)).unwrap();
        let panic = |i: usize| VolumesError::CallbackPanic(format!("p{i}"));
        for i in 0..MAX_HELD_PROBLEMS + 5 {
            stand.fire(vec![panic(i)]);
        }
        // The first one again, with the hold full, is already held.
        stand.fire(vec![panic(0)]);

        let taken = kk.volumes_problems_take();
        assert_eq!(taken.len(), MAX_HELD_PROBLEMS + 1);
        // The first ones are kept, in order.
        for (i, p) in taken[..MAX_HELD_PROBLEMS].iter().enumerate() {
            assert_eq!(p.kind, "CallbackPanic");
            assert!(p.message.ends_with(&format!("p{i}")), "{}", p.message);
        }
        let overflow = &taken[MAX_HELD_PROBLEMS];
        assert_eq!(overflow.kind, "ProblemsNotShown");
        assert!(
            overflow.message.contains("5 more not shown"),
            "{}",
            overflow.message
        );

        // Taking starts the count again.
        assert!(kk.volumes_problems_take().is_empty());
        stand.fire(vec![panic(1000)]);
        assert_eq!(kk.volumes_problems_take().len(), 1);
    }

    /// The problems of starting the watcher are in the first listing, and
    /// are also held until a consumer takes them, so one that reloads after
    /// that listing still gets them; taking clears them.
    #[test]
    fn startup_problems_are_held_until_taken() {
        let (_app, kk) = kk();
        let stand = Stand::new(|| vec![VolumesError::UdisksAbsent]);
        assert!(kk.volumes_problems_take().is_empty());

        let first = kk.volumes_list_with(&|cb| stand.start(cb)).unwrap();
        assert!(kinds(&first).iter().any(|k| k == "UdisksAbsent"));
        let second = kk.volumes_list_with(&|cb| stand.start(cb)).unwrap();
        assert!(!kinds(&second).iter().any(|k| k == "UdisksAbsent"));

        let taken: Vec<_> = kk
            .volumes_problems_take()
            .into_iter()
            .map(|p| p.kind)
            .collect();
        assert_eq!(taken, ["UdisksAbsent"]);
        assert!(kk.volumes_problems_take().is_empty());
    }

    /// What the watcher meets later is sent in the event as before and also
    /// held, so an event that arrives while the webview reloads is not lost.
    #[test]
    fn runtime_problems_are_sent_and_held_until_taken() {
        let (app, kk) = kk();
        let (tx, rx) = mpsc::channel();
        app.listen(EVENT_VOLUMES_CHANGED, move |e| {
            let _ = tx.send(e.payload().to_owned());
        });
        let stand = Stand::new(Vec::new);
        kk.volumes_list_with(&|cb| stand.start(cb)).unwrap();

        // A change without a problem holds nothing.
        stand.fire(Vec::new());
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(kk.volumes_problems_take().is_empty());

        stand.fire(vec![VolumesError::CallbackPanic("x".into())]);
        let sent: VolumesChanged =
            serde_json::from_str(&rx.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap();
        assert_eq!(sent.problems.len(), 1);
        let taken = kk.volumes_problems_take();
        assert_eq!(taken, sent.problems);
        assert!(kk.volumes_problems_take().is_empty());
    }

    /// A problem that comes back unchanged (UDisks2 leaving and returning)
    /// is held once.
    #[test]
    fn an_identical_problem_is_held_once() {
        let (_app, kk) = kk();
        let stand = Stand::new(Vec::new);
        kk.volumes_list_with(&|cb| stand.start(cb)).unwrap();
        for _ in 0..3 {
            stand.fire(vec![VolumesError::UdisksAbsent]);
        }
        stand.fire(vec![VolumesError::CallbackPanic("p".into())]);
        let kinds: Vec<_> = kk
            .volumes_problems_take()
            .into_iter()
            .map(|p| p.kind)
            .collect();
        assert_eq!(kinds, ["UdisksAbsent", "CallbackPanic"]);
    }

    /// Startup problems a listing has not reported yet are settled by taking
    /// them: the next listing does not repeat what the consumer has.
    #[test]
    fn taking_settles_unreported_startup_problems() {
        let (_app, kk) = kk();
        {
            let mut state = lock(&kk.0.volumes);
            let held = WireError::from(VolumesError::UdisksAbsent);
            state.once = vec![held.clone()];
            state.held = vec![held];
        }
        assert_eq!(kk.volumes_problems_take().len(), 1);
        assert!(lock(&kk.0.volumes).once.is_empty());
        let stand = Stand::new(Vec::new);
        assert!(kk.volumes_start(&|cb| stand.start(cb)).is_empty());
    }

    /// Exiting the app stops the watcher; a window closing does not.
    #[test]
    fn app_exit_stops_the_watcher() {
        let (_app, kk) = kk();
        let stand = Stand::new(Vec::new);
        kk.volumes_list_with(&|cb| stand.start(cb)).unwrap();
        kk.shutdown(Some("main"));
        assert!(lock(&kk.0.volumes).watcher.is_some());
        kk.shutdown(None);
        assert!(lock(&kk.0.volumes).watcher.is_none());
    }
}
