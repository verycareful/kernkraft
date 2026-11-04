// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The mountinfo half of the volume watcher against the real kernel (a mount
//! in a private mount namespace). The UDisks2 half is in `udisks_bus.rs`.
//!
//! The mount test cannot mount in this process without root. It starts a
//! child in new user and mount namespaces with `unshare(1)` and watches the
//! child's `/proc/<pid>/mountinfo`, which is the same kind of descriptor
//! `watch` polls for the process itself. The kernel then makes the
//! descriptor ready exactly as it does for a real mount.
//!
//! Where unprivileged user namespaces are not allowed (the CI runner's
//! AppArmor policy), the environment variable `KERN_TEST_SUDO_MOUNTNS=1`
//! makes the test run `sudo -n unshare --mount --propagation private` instead.
//! Only the shell that mounts and unmounts the tmpfs is root; the test and the
//! watcher stay the invoking user, and the root shell's `/proc/<pid>/mountinfo`
//! is world-readable. A refused `unshare` or `sudo` fails the test; nothing
//! falls back to the other form.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::os::fd::OwnedFd;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use kernkraft_bus::BusError;
use kernkraft_volumes::{watch, watch_with, OnChange, VolumesError, DEBOUNCE, DROP_WAIT};

/// Set (to `1`) on CI to make the mount namespace with `sudo`.
const SUDO_ENV: &str = "KERN_TEST_SUDO_MOUNTNS";

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
    /// The shell that mounts: the child itself, or under `sudo` the root
    /// shell it started.
    shell_pid: u32,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    _dir: tempfile::TempDir,
}

impl Sandbox {
    fn start() -> Sandbox {
        let dir = tempfile::tempdir().unwrap();
        // Every read ends the script on end of input, so a dropped sandbox
        // ends the shell (and with it the namespace and its mounts).
        let script = r#"echo "ready $$"; read go || exit 0; mount -t tmpfs tmpfs "$1" || exit 3; echo mounted; read go || exit 0; umount "$1" || exit 4; echo unmounted; read go || exit 0"#;
        let sudo = std::env::var(SUDO_ENV).as_deref() == Ok("1");
        let mut cmd = if sudo {
            let mut c = Command::new("sudo");
            c.args(["-n", "unshare", "--mount", "--propagation", "private"]);
            c
        } else {
            let mut c = Command::new("unshare");
            c.args(["--user", "--map-root-user", "--mount"]);
            c
        };
        let mut child = cmd
            .args(["sh", "-c", script, "sh"])
            .arg(dir.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("unshare(1) from util-linux (and sudo with KERN_TEST_SUDO_MOUNTNS=1) is needed for this test");
        let stdin = child.stdin.take().unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        let shell_pid = line
            .trim()
            .strip_prefix("ready ")
            .and_then(|p| p.parse().ok())
            .unwrap_or_else(|| {
                panic!(
                    "the child did not start (got {line:?}); it needs unprivileged user \
                     namespaces, or sudo -n with {SUDO_ENV}=1"
                )
            });
        Sandbox {
            child,
            shell_pid,
            stdin,
            stdout,
            _dir: dir,
        }
    }

    fn mountinfo(&self) -> OwnedFd {
        OwnedFd::from(File::open(format!("/proc/{}/mountinfo", self.shell_pid)).unwrap())
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
    // At most the system bus or UDisks2 being absent. A refused subscription
    // or an I/O failure of the mount half is a defect, not an environment.
    assert!(problems.len() <= 1, "{problems:?}");
    for p in &problems {
        assert!(
            matches!(p, VolumesError::Bus(_) | VolumesError::UdisksAbsent),
            "{p:?}"
        );
    }
    let t = Instant::now();
    drop(watcher);
    assert!(t.elapsed() < DROP_WAIT / 2);
}
