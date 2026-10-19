// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

/// Every failure `kernkraft-bus` reports.
#[derive(Debug, thiserror::Error)]
pub enum BusError {
    /// No address could be built: the bus variable is unset (or empty) and
    /// there is no `XDG_RUNTIME_DIR` to derive the default socket from.
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
    /// The socket could not be opened. `defaulted` is true when the address
    /// was not given by the environment but derived from the default
    /// location, so a missing socket there means "no bus on this machine".
    #[error("connecting to {target}: {source}")]
    Connect {
        target: String,
        defaulted: bool,
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

impl BusError {
    /// Whether the error only says the bus is not there: no address could be
    /// built, or the default socket does not exist. An address the
    /// environment names explicitly that cannot be reached is not absence.
    pub fn is_absent(&self) -> bool {
        match self {
            BusError::NoAddress(_) => true,
            BusError::Connect {
                defaulted, source, ..
            } => *defaulted && source.kind() == std::io::ErrorKind::NotFound,
            _ => false,
        }
    }
}
