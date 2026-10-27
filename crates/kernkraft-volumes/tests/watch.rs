// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The volume watcher against the real kernel (a mount in a private mount
//! namespace) and against a fake bus daemon on a socket pair.
//!
//! The mount test cannot mount in this process without root. It starts a
//! child in new user and mount namespaces with `unshare(1)` and watches the
//! child's `/proc/<pid>/mountinfo`, which is the same kind of descriptor
//! `watch` polls for the process itself. The kernel then makes the
//! descriptor ready exactly as it does for a real mount.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use kernkraft_bus::BusError;
use kernkraft_volumes::{watch, watch_with, OnChange, VolumesError, DEBOUNCE, DROP_WAIT};
use zbus::blocking::connection::Builder;
use zbus::blocking::Connection;

fn collector() -> (OnChange, mpsc::Receiver<Vec<VolumesError>>) {
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    (
        Box::new(move |e| {
            let _ = tx.lock().unwrap().send(e);
        }),
        rx,
    )
}

fn own_mountinfo() -> OwnedFd {
    OwnedFd::from(File::open("/proc/self/mountinfo").unwrap())
}

/// A child in new user and mount namespaces that mounts and unmounts a
/// tmpfs when told to, announcing each step on its stdout.
struct Sandbox {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    _dir: tempfile::TempDir,
}

impl Sandbox {
    fn start() -> Sandbox {
        let dir = tempfile::tempdir().unwrap();
        let script = r#"echo ready; read go; mount -t tmpfs tmpfs "$1" || exit 3; echo mounted; read go; umount "$1" || exit 4; echo unmounted; read go"#;
        let mut child = Command::new("unshare")
            .args([
                "--user",
                "--map-root-user",
                "--mount",
                "sh",
                "-c",
                script,
                "sh",
            ])
            .arg(dir.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("unshare(1) from util-linux is needed for this test");
        let stdin = child.stdin.take().unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        assert_eq!(
            line.trim(),
            "ready",
            "the child could not create user and mount namespaces; \
             unprivileged user namespaces must be allowed"
        );
        Sandbox {
            child,
            stdin,
            stdout,
            _dir: dir,
        }
    }

    fn mountinfo(&self) -> OwnedFd {
        OwnedFd::from(File::open(format!("/proc/{}/mountinfo", self.child.id())).unwrap())
    }

    /// Lets the child do its next step and waits for its announcement.
    fn step(&mut self, expect: &str) {
        self.stdin.write_all(b"go\n").unwrap();
        self.stdin.flush().unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        assert_eq!(line.trim(), expect);
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn absent_bus() -> BusError {
    BusError::Connect {
        target: "/run/dbus/system_bus_socket".into(),
        defaulted: true,
        source: std::io::Error::from(std::io::ErrorKind::NotFound),
    }
}

/// A mount and an unmount each make the polled descriptor ready; nothing
/// else does.
#[test]
fn mount_and_unmount_call_back() {
    let mut sb = Sandbox::start();
    let (cb, rx) = collector();
    let (_watcher, problems) = watch_with(sb.mountinfo(), Err(absent_bus()), cb).unwrap();
    assert_eq!(problems.len(), 1);

    // Quiet until something happens.
    assert!(rx.recv_timeout(DEBOUNCE * 3).is_err());

    sb.step("mounted");
    let errors = rx.recv_timeout(Duration::from_secs(5)).expect("mount");
    assert!(errors.is_empty(), "{errors:?}");
    assert!(
        rx.recv_timeout(DEBOUNCE * 3).is_err(),
        "one callback per mount"
    );

    sb.step("unmounted");
    let errors = rx.recv_timeout(Duration::from_secs(5)).expect("unmount");
    assert!(errors.is_empty(), "{errors:?}");
}

/// The watcher's own process: its descriptor stays quiet when nothing is
/// mounted, and dropping it returns promptly.
#[test]
fn quiet_descriptor_and_prompt_drop() {
    let (cb, rx) = collector();
    let (watcher, _) = watch_with(own_mountinfo(), Err(absent_bus()), cb).unwrap();
    assert!(rx.recv_timeout(DEBOUNCE * 3).is_err());
    let t = Instant::now();
    drop(watcher);
    assert!(t.elapsed() < DROP_WAIT / 2);
    // The callback is gone with the worker: the channel disconnects.
    assert!(matches!(
        rx.recv_timeout(DEBOUNCE),
        Err(mpsc::RecvTimeoutError::Disconnected)
    ));
}

/// The standard entry point opens the process's own mountinfo and starts
/// on this machine, whatever it has on its system bus.
#[test]
fn live_watch_starts_and_stops() {
    let (cb, _rx) = collector();
    let (watcher, problems) = watch(cb).unwrap();
    // Any problem is one of the documented absences or a refused
    // subscription, never an I/O failure of the mount half.
    for p in &problems {
        assert!(
            matches!(
                p,
                VolumesError::Bus(_) | VolumesError::UdisksAbsent | VolumesError::Dbus { .. }
            ),
            "{p:?}"
        );
    }
    let t = Instant::now();
    drop(watcher);
    assert!(t.elapsed() < DROP_WAIT / 2);
}

/// What UDisks' side of the bus daemon answers the watcher's lookups with.
struct FakeBus {
    owned: bool,
    activatable: Vec<String>,
}

#[zbus::interface(name = "org.freedesktop.DBus")]
impl FakeBus {
    fn name_has_owner(&self, name: &str) -> bool {
        assert_eq!(name, "org.freedesktop.UDisks2");
        self.owned
    }

    fn list_activatable_names(&self) -> Vec<String> {
        self.activatable.clone()
    }

    fn add_match(&self, _rule: &str) {}
}

fn fake_bus(bus: FakeBus) -> Connection {
    let (a, b) = UnixStream::pair().unwrap();
    let guid = zbus::Guid::generate();
    let server = std::thread::spawn(move || {
        Builder::async_io_unix_stream(a)
            .p2p()
            .server(guid)
            .unwrap()
            .serve_at("/org/freedesktop/DBus", bus)
            .unwrap()
            .build()
            .unwrap()
    });
    let client = Builder::async_io_unix_stream(b).p2p().build().unwrap();
    // Keep the server connection alive for the test's duration.
    std::mem::forget(server.join().unwrap());
    client
}

/// A bus without UDisks2, running or activatable, is reported once and the
/// mount watch is up.
#[test]
fn absent_udisks_is_reported_once() {
    let (cb, _rx) = collector();
    let bus = fake_bus(FakeBus {
        owned: false,
        activatable: vec!["org.freedesktop.other".into()],
    });
    let (watcher, problems) = watch_with(own_mountinfo(), Ok(bus), cb).unwrap();
    assert!(
        matches!(problems[..], [VolumesError::UdisksAbsent]),
        "{problems:?}"
    );
    drop(watcher);
}

/// A bus that is not there at all is reported once, as the bus error.
#[test]
fn absent_bus_is_reported_once() {
    let (cb, _rx) = collector();
    let (watcher, problems) = watch_with(own_mountinfo(), Err(absent_bus()), cb).unwrap();
    match &problems[..] {
        [VolumesError::Bus(e)] => assert!(e.is_absent()),
        other => panic!("{other:?}"),
    }
    drop(watcher);
}

/// UDisks2 on the bus (here only activatable) is subscribed to without a
/// problem, and dropping the watcher ends the signal reader.
#[test]
fn present_udisks_subscribes_and_drops() {
    for (owned, activatable) in [
        (true, vec![]),
        (false, vec!["org.freedesktop.UDisks2".to_string()]),
    ] {
        let (cb, _rx) = collector();
        let bus = fake_bus(FakeBus { owned, activatable });
        let (watcher, problems) = watch_with(own_mountinfo(), Ok(bus), cb).unwrap();
        assert!(problems.is_empty(), "{problems:?}");
        let t = Instant::now();
        drop(watcher);
        assert!(
            t.elapsed() < DROP_WAIT / 2,
            "drop waited for the signal reader"
        );
    }
}
