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

use zbus::blocking::connection::Builder;
use zbus::blocking::Connection;

#[derive(Debug, thiserror::Error)]
pub enum BusError {
    #[error("no bus address: {0} is unset and no default socket path can be built")]
    NoAddress(&'static str),
    #[error(
        "bus address {0:?} has no unix:path= or unix:abstract= entry (only Unix sockets are used)"
    )]
    NotUnix(String),
    #[error("bus address {address:?}: {reason}")]
    Malformed {
        address: String,
        reason: &'static str,
    },
    #[error("connecting to {target}: {source}")]
    Connect {
        target: String,
        #[source]
        source: std::io::Error,
    },
    #[error("D-Bus handshake on {target}: {source}")]
    Handshake {
        target: String,
        #[source]
        source: Box<zbus::Error>,
    },
}

/// Where a Unix bus socket lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnixTarget {
    Path(PathBuf),
    /// Linux abstract namespace name, without the leading NUL.
    Abstract(Vec<u8>),
}

/// Decodes D-Bus address value escaping: `%` followed by two hex digits is
/// that byte (Server Addresses: "backslash... optionally-escaped bytes").
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
                .ok_or_else(|| bad("truncated % escape"))?;
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

/// Picks the first `unix:` entry of a semicolon-separated address list.
/// Entries of other transports are skipped; if none is a Unix entry the
/// error names the whole list.
pub fn parse_address(list: &str) -> Result<UnixTarget, BusError> {
    for entry in list.split(';').map(str::trim).filter(|e| !e.is_empty()) {
        let Some(params) = entry.strip_prefix("unix:") else {
            continue;
        };
        let mut found = None;
        for kv in params.split(',') {
            let Some((k, v)) = kv.split_once('=') else {
                continue;
            };
            let target = match k {
                "path" => UnixTarget::Path(PathBuf::from(std::ffi::OsStr::from_bytes(&unescape(
                    v, list,
                )?))),
                "abstract" => UnixTarget::Abstract(unescape(v, list)?),
                _ => continue,
            };
            if found.is_some() {
                return Err(BusError::Malformed {
                    address: list.to_string(),
                    reason: "unix entry has more than one of path and abstract",
                });
            }
            found = Some(target);
        }
        // A unix entry with neither key (for example `unix:runtime=yes`)
        // names no socket; skip it like any entry that cannot be used.
        if let Some(t) = found {
            return Ok(t);
        }
    }
    Err(BusError::NotUnix(list.to_string()))
}

fn address_for(
    var: &'static str,
    default: impl FnOnce() -> Option<String>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<String, BusError> {
    env(var)
        .filter(|v| !v.is_empty())
        .or_else(default)
        .ok_or(BusError::NoAddress(var))
}

/// The session bus address: `DBUS_SESSION_BUS_ADDRESS`, else
/// `unix:path=$XDG_RUNTIME_DIR/bus` (the systemd/dbus-broker default).
pub fn session_address(env: &dyn Fn(&str) -> Option<String>) -> Result<String, BusError> {
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
pub fn system_address(env: &dyn Fn(&str) -> Option<String>) -> Result<String, BusError> {
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

fn open(address: &str) -> Result<Connection, BusError> {
    let target = parse_address(address)?;
    let (label, stream) = match &target {
        UnixTarget::Path(p) => (p.display().to_string(), UnixStream::connect(p)),
        UnixTarget::Abstract(name) => (
            format!("@{}", String::from_utf8_lossy(name)),
            SocketAddr::from_abstract_name(name).and_then(|a| UnixStream::connect_addr(&a)),
        ),
    };
    let stream = stream.map_err(|source| BusError::Connect {
        target: label.clone(),
        source,
    })?;
    #[allow(deprecated)] // the std stream form; the alternative needs async-io types
    Builder::unix_stream(stream)
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
