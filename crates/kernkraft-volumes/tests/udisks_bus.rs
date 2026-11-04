// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The UDisks2 half of the volume watcher against a private `dbus-daemon`,
//! reached through `kernkraft_bus::system()` exactly as in production: the
//! daemon listens on an abstract Unix socket (a path under the repository
//! would exceed the 107-byte `sun_path` limit) and `DBUS_SYSTEM_BUS_ADDRESS`
//! points at it. That variable is the one address source the production path
//! reads, and setting it is only safe while no other thread can read the
//! environment, so this file has a single test: it sets the variable before
//! it opens a connection (which spawns threads), and restores it after the
//! scenarios, each of which has dropped its connections by then. The test
//! takes the name `org.freedesktop.UDisks2` on the bus and emits the signals
//! UDisks2 documents, in turn for each scenario.
//!
//! `dbus-daemon` must be installed; a missing binary fails the test.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::os::fd::OwnedFd;
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Mutex};
use std::time::Duration;

use kernkraft_volumes::{watch_with, OnChange, VolumeWatcher, VolumesError, DEBOUNCE};
use zbus::blocking::Connection;
use zbus::zvariant::{ObjectPath, Value};

const SERVICE: &str = "org.freedesktop.UDisks2";
const ROOT: &str = "/org/freedesktop/UDisks2";
const OBJECT_MANAGER: &str = "org.freedesktop.DBus.ObjectManager";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
const BLOCK: &str = "org.freedesktop.UDisks2.Block";
const JOB: &str = "org.freedesktop.UDisks2.Job";
const SDB1: &str = "/org/freedesktop/UDisks2/block_devices/sdb1";
const WAIT: Duration = Duration::from_secs(10);

/// Sets `DBUS_SYSTEM_BUS_ADDRESS` and puts it back when dropped.
struct EnvGuard {
    saved: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn set(address: &str) -> EnvGuard {
        let saved = std::env::var_os("DBUS_SYSTEM_BUS_ADDRESS");
        std::env::set_var("DBUS_SYSTEM_BUS_ADDRESS", address);
        EnvGuard { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.saved.take() {
            Some(v) => std::env::set_var("DBUS_SYSTEM_BUS_ADDRESS", v),
            None => std::env::remove_var("DBUS_SYSTEM_BUS_ADDRESS"),
        }
    }
}

struct Daemon {
    child: Child,
    address: String,
    _dir: tempfile::TempDir,
}

impl Daemon {
    fn start() -> Daemon {
        let dir = tempfile::tempdir().unwrap();
        let name = format!("kernkraft-volumes-test-{}", std::process::id());
        let config = dir.path().join("bus.conf");
        std::fs::write(
            &config,
            format!(
                r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:abstract={name}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#
            ),
        )
        .unwrap();
        let mut child = Command::new("dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .args(["--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("dbus-daemon is needed for this test");
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let address = line.trim().to_owned();
        assert!(address.starts_with("unix:abstract="), "address {address:?}");
        Daemon {
            child,
            address,
            _dir: dir,
        }
    }

    /// A connection to this daemon through `kernkraft_bus::system()`; the
    /// environment must point at it (see [`EnvGuard`]).
    fn connect(&self) -> Result<Connection, kernkraft_bus::BusError> {
        kernkraft_bus::system()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

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

fn start_watch(
    daemon: &Daemon,
) -> (
    VolumeWatcher,
    Vec<VolumesError>,
    mpsc::Receiver<Vec<VolumesError>>,
    Connection,
) {
    let (cb, rx) = collector();
    let conn = daemon.connect();
    let handle = conn.as_ref().unwrap().clone();
    let fd = OwnedFd::from(File::open("/proc/self/mountinfo").unwrap());
    let (w, problems) = watch_with(fd, conn, cb).unwrap();
    (w, problems, rx, handle)
}

fn added(conn: &Connection, dest: Option<&str>, path: &str, iface: &str) {
    let body: HashMap<String, HashMap<String, Value<'_>>> =
        HashMap::from([(iface.to_string(), HashMap::new())]);
    conn.emit_signal(
        dest,
        ROOT,
        OBJECT_MANAGER,
        "InterfacesAdded",
        &(ObjectPath::try_from(path).unwrap(), body),
    )
    .unwrap();
}

fn removed(conn: &Connection, path: &str, iface: &str) {
    conn.emit_signal(
        None::<&str>,
        ROOT,
        OBJECT_MANAGER,
        "InterfacesRemoved",
        &(ObjectPath::try_from(path).unwrap(), vec![iface.to_string()]),
    )
    .unwrap();
}

fn expect_call(rx: &mpsc::Receiver<Vec<VolumesError>>, what: &str) -> Vec<VolumesError> {
    rx.recv_timeout(WAIT)
        .unwrap_or_else(|_| panic!("no callback for {what}"))
}

fn expect_quiet(rx: &mpsc::Receiver<Vec<VolumesError>>, what: &str) {
    if let Ok(e) = rx.recv_timeout(DEBOUNCE * 4) {
        panic!("callback for {what}: {e:?}");
    }
}

/// One daemon, one environment setting, the scenarios in turn. Each ends
/// with every name released, so the next starts from a bus UDisks2 is not on.
#[test]
fn private_bus() {
    // Locals drop in reverse order of declaration: `_env` puts the variable
    // back first, then `daemon` is killed. The daemon is another process, so
    // it does not matter that it outlives the variable; what matters is that
    // the restore comes after the scenarios, which own every connection (and
    // so every thread of this test that could read the environment) and have
    // dropped them.
    let daemon = Daemon::start();
    let _env = EnvGuard::set(&daemon.address);
    udisks_signals_call_back(&daemon);
    udisks_arrival_departure_and_return(&daemon);
}

/// UDisks2 present from the start: its Block and Filesystem signals call
/// back, its other signals and anyone else's do not.
fn udisks_signals_call_back(daemon: &Daemon) {
    let udisks = daemon.connect().unwrap();
    udisks.request_name(SERVICE).unwrap();
    let (watcher, problems, rx, handle) = start_watch(daemon);
    assert!(problems.is_empty(), "{problems:?}");

    added(&udisks, None, SDB1, BLOCK);
    assert!(expect_call(&rx, "InterfacesAdded").is_empty());
    expect_quiet(&rx, "one callback per signal burst");

    removed(&udisks, SDB1, BLOCK);
    assert!(expect_call(&rx, "InterfacesRemoved").is_empty());

    udisks
        .emit_signal(
            None::<&str>,
            SDB1,
            PROPERTIES,
            "PropertiesChanged",
            &(
                BLOCK.to_string(),
                HashMap::from([("IdLabel".to_string(), Value::from("x"))]),
                Vec::<String>::new(),
            ),
        )
        .unwrap();
    expect_call(&rx, "PropertiesChanged");

    // A Job object is not a volume.
    added(&udisks, None, "/org/freedesktop/UDisks2/jobs/1", JOB);
    expect_quiet(&rx, "a Job");

    // Another client's broadcast never reaches the watcher: the bus matches
    // the sender's name.
    let other = daemon.connect().unwrap();
    added(&other, None, SDB1, BLOCK);
    expect_quiet(&rx, "a broadcast from another sender");

    // Nor does a signal another client sends straight to the watcher.
    let watcher_name = handle.unique_name().unwrap().to_string();
    added(&other, Some(&watcher_name), SDB1, BLOCK);
    expect_quiet(&rx, "a unicast from another sender");

    drop(watcher);
    udisks.release_name(SERVICE).unwrap();
}

/// UDisks2 absent at the start is reported once; arriving later, leaving and
/// coming back are followed.
fn udisks_arrival_departure_and_return(daemon: &Daemon) {
    let (watcher, problems, rx, _handle) = start_watch(daemon);
    assert!(
        matches!(problems[..], [VolumesError::UdisksAbsent]),
        "{problems:?}"
    );
    expect_quiet(&rx, "the start");

    // Arrives late.
    let first = daemon.connect().unwrap();
    first.request_name(SERVICE).unwrap();
    assert!(expect_call(&rx, "arrival").is_empty());
    added(&first, None, SDB1, BLOCK);
    expect_call(&rx, "a signal from the new owner");

    // Leaves: a change, reported once.
    first.release_name(SERVICE).unwrap();
    let errors = expect_call(&rx, "departure");
    assert!(
        matches!(errors[..], [VolumesError::UdisksAbsent]),
        "{errors:?}"
    );
    expect_quiet(&rx, "departure reported once");
    // Its signals no longer count.
    added(&first, None, SDB1, BLOCK);
    expect_quiet(&rx, "a signal from a name no longer owned");

    // Returns, as another process.
    let second = daemon.connect().unwrap();
    second.request_name(SERVICE).unwrap();
    assert!(expect_call(&rx, "return").is_empty());
    added(&second, None, SDB1, BLOCK);
    expect_call(&rx, "a signal from the returned owner");

    drop(watcher);
}
