// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use kernkraft_bus::{parse_address, session_address, system_address, BusError, UnixTarget};

#[test]
fn bus_address_parsing() {
    assert_eq!(
        parse_address("unix:path=/run/user/1000/bus").unwrap(),
        UnixTarget::Path(PathBuf::from("/run/user/1000/bus"))
    );
    assert_eq!(
        parse_address("unix:abstract=/tmp/dbus-XYZ,guid=abc").unwrap(),
        UnixTarget::Abstract(b"/tmp/dbus-XYZ".to_vec())
    );
    // %20 is a space, %25 a percent sign, %ff a non-UTF-8 byte.
    assert_eq!(
        parse_address("unix:path=/a%20b%25c").unwrap(),
        UnixTarget::Path(PathBuf::from("/a b%c"))
    );
    assert_eq!(
        parse_address("unix:abstract=x%ffy").unwrap(),
        UnixTarget::Abstract(vec![b'x', 0xff, b'y'])
    );
    // First unix entry wins; a tcp entry before it is skipped.
    assert_eq!(
        parse_address("tcp:host=example.com,port=1234;unix:path=/s;unix:path=/t").unwrap(),
        UnixTarget::Path(PathBuf::from("/s"))
    );
    for refused in [
        "tcp:host=127.0.0.1,port=1",
        "nonce-tcp:host=127.0.0.1,port=1,noncefile=/x",
        "launchd:env=DBUS",
        "tcp:host=a;launchd:env=B",
        "unix:guid=abc",
        "",
        ";",
    ] {
        assert!(
            matches!(parse_address(refused), Err(BusError::NotUnix(_))),
            "{refused:?}"
        );
    }
    for malformed in [
        "unix:path=/a%2",
        "unix:path=/a%zz",
        "unix:path=/a,abstract=b",
    ] {
        assert!(
            matches!(parse_address(malformed), Err(BusError::Malformed { .. })),
            "{malformed:?}"
        );
    }
}

#[test]
fn default_addresses() {
    let none = |_: &str| None;
    let env = |k: &str| match k {
        "XDG_RUNTIME_DIR" => Some("/run/user/1000".to_string()),
        _ => None,
    };
    assert_eq!(
        session_address(&env).unwrap(),
        "unix:path=/run/user/1000/bus"
    );
    assert!(matches!(
        session_address(&none),
        Err(BusError::NoAddress(_))
    ));
    // An empty variable counts as unset; a relative runtime dir is unusable.
    let empty = |k: &str| (k == "DBUS_SESSION_BUS_ADDRESS").then(String::new);
    assert!(session_address(&empty).is_err());
    let set = |k: &str| (k == "DBUS_SESSION_BUS_ADDRESS").then(|| "tcp:host=a".to_string());
    assert_eq!(session_address(&set).unwrap(), "tcp:host=a");
    assert_eq!(
        system_address(&none).unwrap(),
        "unix:path=/run/dbus/system_bus_socket"
    );
}

/// Needs a session bus; returns early without one.
#[test]
fn live_session_connects_over_unix() {
    let Ok(conn) = kernkraft_bus::session() else {
        eprintln!("no session bus: skipping");
        return;
    };
    assert!(conn.unique_name().is_some());
}
