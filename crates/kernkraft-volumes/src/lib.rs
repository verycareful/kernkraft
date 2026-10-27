// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Mounted volumes and UDisks2 mount, unmount and eject.
//!
//! - [`list`] reads `/proc/self/mountinfo` (proc(5), "/proc/pid/mountinfo")
//!   and keeps the mounts a user would call volumes; [`parse_mountinfo`] and
//!   [`user_mounts`] are the pure halves of it.
//! - [`Udisks`] mounts, unmounts and ejects through the UDisks2 D-Bus API
//!   (<https://storaged.org/doc/udisks2-api/latest/>) on the system bus,
//!   which `kernkraft-bus` opens over its Unix socket.
//! - [`watch`] calls back when the set of volumes may have changed: a mount
//!   or unmount (`/proc/self/mountinfo`) or a device appearing or going away
//!   (UDisks2 signals).
//!
//! Mount points cross this API as `PathBuf` holding the exact bytes; names
//! shown to people (`device`, `label`) are `String`s and may have had
//! invalid UTF-8 replaced. [`Device`] is plain data and derives serde;
//! [`Mount`] holds a `PathBuf`, which serde cannot carry losslessly, so it
//! has no serde impls and an adapter converts its path.

mod error;
mod mountinfo;
mod udisks;
mod watch;

pub use error::{EjectRefusal, VolumesError};
pub use mountinfo::{
    decode_udev_name, list, parse_mountinfo, parse_mountinfo_report, parse_mountinfo_strict,
    user_mounts, ListProblem, Listing, Mount, MountInfoLine,
};
pub use udisks::{Device, Udisks, ACTION_TIMEOUT};
pub use watch::{watch, watch_with, OnChange, VolumeWatcher, DEBOUNCE, DROP_WAIT};
