// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! D-Bus connections over a Unix socket and nothing else.
//!
//! zbus compiles in a TCP transport that a `tcp:` bus address would reach, so
//! no kernkraft crate lets zbus resolve an address. This crate resolves it
//! itself (D-Bus Specification, "Server Addresses" and "Well-known Message
//! Bus Instances"), accepts only `unix:path=` and `unix:abstract=`, opens the
//! `UnixStream` here and hands that stream to zbus. Any other transport is
//! an error naming it, never a fallback.

use std::os::linux::net::SocketAddrExt;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::{SocketAddr, UnixStream};
use std::path::PathBuf;

mod error;

pub use error::BusError;

use std::time::Duration;

use zbus::blocking::connection::Builder;
use zbus::blocking::Connection;

/// Where a Unix bus socket lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnixTarget {
    Path(PathBuf),
    /// Linux abstract namespace name, without the leading NUL.
    Abstract(Vec<u8>),
}

/// Longest a method call may wait for its reply. zbus waits forever by
/// default, so a hung peer (a stuck xdg-desktop-portal) would stall every
/// caller; a healthy bus answers in milliseconds, and 3 s leaves room for a
/// portal that is starting up.
pub const METHOD_TIMEOUT: Duration = Duration::from_secs(3);

/// Decodes D-Bus address value escaping: `%` followed by two hex digits is
/// that byte (Server Addresses, "optionally-escaped bytes").
fn unescape(value: &str, address: &str) -> Result<Vec<u8>, BusError> {
    let bad = |reason| BusError::Malformed {
        address: address.to_string(),
        reason,
    };
    let b = value.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = b
                .get(i + 1..i + 3)
                .filter(|h| h.iter().all(u8::is_ascii_hexdigit))
                .ok_or_else(|| bad("bad % escape"))?;
            // Both bytes are ASCII hex digits, so this is valid UTF-8.
            let s = std::str::from_utf8(hex).map_err(|_| bad("bad % escape"))?;
            out.push(u8::from_str_radix(s, 16).map_err(|_| bad("bad % escape"))?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    Ok(out)
}

/// Picks the first usable `unix:` entry of a semicolon-separated address
/// list. Entries of other transports are skipped on purpose, so a list that
/// offers `tcp:` first and `unix:` second connects over the Unix socket; if
/// no entry is a Unix one the error names the whole list. A `unix:` entry
/// must give exactly one of `path` and `abstract` (non-empty), every
/// parameter needs `=`, and `dir`, `tmpdir` and `runtime` (which name a
/// socket to listen on, not one to connect to) cannot accompany them.
pub fn parse_address(list: &str) -> Result<UnixTarget, BusError> {
    let bad = |reason| BusError::Malformed {
        address: list.to_string(),
        reason,
    };
    for entry in list.split(';').map(str::trim).filter(|e| !e.is_empty()) {
        let Some(params) = entry.strip_prefix("unix:") else {
            continue;
        };
        let mut found = None;
        let mut listen_only = false;
        for kv in params.split(',') {
            let (k, v) = kv
                .split_once('=')
                .ok_or_else(|| bad("parameter without ="))?;
            let target = match k {
                "path" | "abstract" => {
                    if v.is_empty() {
                        return Err(bad("empty socket name"));
                    }
                    let name = unescape(v, list)?;
                    if k == "path" {
                        UnixTarget::Path(PathBuf::from(std::ffi::OsStr::from_bytes(&name)))
                    } else {
                        UnixTarget::Abstract(name)
                    }
                }
                "dir" | "tmpdir" | "runtime" => {
                    listen_only = true;
                    continue;
                }
                _ => continue,
            };
            if found.is_some() {
                return Err(bad("more than one of path and abstract"));
            }
            found = Some(target);
        }
        match found {
            Some(_) if listen_only => {
                return Err(bad("path or abstract together with dir, tmpdir or runtime"))
            }
            Some(t) => return Ok(t),
            // A listen-only or parameterless entry names nothing to connect
            // to; like any other unusable entry it is skipped.
            None => {}
        }
    }
    Err(BusError::NotUnix(list.to_string()))
}

/// A resolved bus address and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BusAddress {
    pub address: String,
    /// True when the environment named no address and this is the default
    /// socket location.
    pub defaulted: bool,
}

fn address_for(
    var: &'static str,
    default: impl FnOnce() -> Option<String>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<BusAddress, BusError> {
    match env(var).filter(|v| !v.is_empty()) {
        Some(address) => Ok(BusAddress {
            address,
            defaulted: false,
        }),
        None => default()
            .map(|address| BusAddress {
                address,
                defaulted: true,
            })
            .ok_or(BusError::NoAddress(var)),
    }
}

/// The session bus address: `DBUS_SESSION_BUS_ADDRESS`, else
/// `unix:path=$XDG_RUNTIME_DIR/bus` (the systemd/dbus-broker default).
pub fn session_address(env: &dyn Fn(&str) -> Option<String>) -> Result<BusAddress, BusError> {
    address_for(
        "DBUS_SESSION_BUS_ADDRESS",
        || {
            env("XDG_RUNTIME_DIR")
                .filter(|d| d.starts_with('/'))
                .map(|d| format!("unix:path={}/bus", escape(&d)))
        },
        env,
    )
}

/// The system bus address: `DBUS_SYSTEM_BUS_ADDRESS`, else
/// `unix:path=/run/dbus/system_bus_socket` (Well-known Message Bus Instances).
pub fn system_address(env: &dyn Fn(&str) -> Option<String>) -> Result<BusAddress, BusError> {
    address_for(
        "DBUS_SYSTEM_BUS_ADDRESS",
        || Some("unix:path=/run/dbus/system_bus_socket".to_string()),
        env,
    )
}

/// Escapes bytes that are not allowed unescaped in an address value
/// (optionally-escaped: alphanumerics and `-_/.\*`).
fn escape(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_/.\\*".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02x}"));
        }
    }
    out
}

fn open(addr: &BusAddress) -> Result<Connection, BusError> {
    let target = parse_address(&addr.address)?;
    let (label, stream) = match &target {
        UnixTarget::Path(p) => (p.display().to_string(), UnixStream::connect(p)),
        UnixTarget::Abstract(name) => (
            format!("@{}", String::from_utf8_lossy(name)),
            SocketAddr::from_abstract_name(name).and_then(|a| UnixStream::connect_addr(&a)),
        ),
    };
    let stream = stream.map_err(|source| BusError::Connect {
        target: label.clone(),
        defaulted: addr.defaulted,
        source,
    })?;
    Builder::async_io_unix_stream(stream)
        .method_timeout(METHOD_TIMEOUT)
        .build()
        .map_err(|source| BusError::Handshake {
            target: label,
            source: Box::new(source),
        })
}

fn process_env(k: &str) -> Option<String> {
    std::env::var(k).ok()
}

/// Connects to the session bus over its Unix socket.
pub fn session() -> Result<Connection, BusError> {
    open(&session_address(&process_env)?)
}

/// Connects to the system bus over its Unix socket.
pub fn system() -> Result<Connection, BusError> {
    open(&system_address(&process_env)?)
}
