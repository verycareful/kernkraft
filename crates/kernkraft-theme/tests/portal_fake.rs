// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! `read_portal` against a fake Settings service on a peer-to-peer socket
//! pair: no session bus and no real portal.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::os::unix::net::UnixStream;

use kernkraft_theme::{read_portal, Portal, Rgba, Scheme};
use zbus::blocking::connection::Builder;
use zbus::blocking::Connection;
use zbus::zvariant::{OwnedValue, StructureBuilder, Value};

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.portal.Error")]
enum PortalError {
    #[zbus(error)]
    ZBus(zbus::Error),
    NotFound(String),
}

fn lookup(key: &str) -> Result<Value<'static>, PortalError> {
    match key {
        "color-scheme" => Ok(Value::U32(1)),
        "accent-color" => Ok(Value::Structure(
            StructureBuilder::new()
                .add_field(1.0f64)
                .add_field(0.0f64)
                .add_field(0.5f64)
                .build()
                .unwrap(),
        )),
        "contrast" => Ok(Value::U32(1)),
        other => Err(PortalError::NotFound(format!("no key {other}"))),
    }
}

/// Settings version 2: `ReadOne` returns the bare value. `contrast` is
/// missing, to exercise the NotFound path.
struct V2;

#[zbus::interface(name = "org.freedesktop.portal.Settings")]
impl V2 {
    fn read_one(&self, namespace: &str, key: &str) -> Result<OwnedValue, PortalError> {
        assert_eq!(namespace, "org.freedesktop.appearance");
        if key == "contrast" {
            return Err(PortalError::NotFound("contrast".into()));
        }
        Ok(OwnedValue::try_from(lookup(key)?).unwrap())
    }
}

/// Settings version 1: only `Read`, whose reply wraps the value in one more
/// variant. Every key exists.
struct V1;

#[zbus::interface(name = "org.freedesktop.portal.Settings")]
impl V1 {
    fn read(&self, namespace: &str, key: &str) -> Result<OwnedValue, PortalError> {
        assert_eq!(namespace, "org.freedesktop.appearance");
        let inner = lookup(key)?;
        Ok(OwnedValue::try_from(Value::Value(Box::new(inner))).unwrap())
    }
}

/// A client connection to `service`, served on the other end of a socket
/// pair.
fn connect<S: zbus::object_server::Interface>(service: S) -> Connection {
    let (a, b) = UnixStream::pair().unwrap();
    let guid = zbus::Guid::generate();
    let server = std::thread::spawn(move || {
        Builder::async_io_unix_stream(a)
            .p2p()
            .server(guid)
            .unwrap()
            .serve_at("/org/freedesktop/portal/desktop", service)
            .unwrap()
            .build()
            .unwrap()
    });
    let client = Builder::async_io_unix_stream(b).p2p().build().unwrap();
    // Keep the server connection alive for the test's duration.
    std::mem::forget(server.join().unwrap());
    client
}

#[test]
fn read_one_with_missing_key() {
    let p = read_portal(&connect(V2)).unwrap().unwrap();
    assert_eq!(
        p,
        Portal {
            color_scheme: Some(Scheme::Dark),
            // 1.0, 0.0, 0.5 of 255; 127.5 rounds up.
            accent: Some(Rgba::rgb(255, 0, 128)),
            // `contrast` is NotFound: unset, not an error.
            high_contrast: false,
        }
    );
}

#[test]
fn version_1_falls_back_to_read() {
    let p = read_portal(&connect(V1)).unwrap().unwrap();
    assert_eq!(p.color_scheme, Some(Scheme::Dark));
    assert_eq!(p.accent, Some(Rgba::rgb(255, 0, 128)));
    assert!(p.high_contrast);
}
