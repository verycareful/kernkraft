// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

use std::path::{Path, PathBuf};

use kernkraft_bus::BusError;

/// Every failure `kernkraft-volumes` reports.
#[derive(Debug, thiserror::Error)]
pub enum VolumesError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// A mountinfo line (1-based) that does not follow proc(5). The kernel
    /// writes every line itself, so this means the format is not what this
    /// crate assumes; it is reported instead of hiding the volume it
    /// describes.
    #[error("mountinfo line {line}: {reason}")]
    MalformedMountinfo { line: usize, reason: &'static str },
    /// Neither the passwd database nor `$USER` names the user, so the
    /// `/run/media/<user>` directory cannot be told.
    #[error("no user name for uid {uid}: not in /etc/passwd and $USER is unset")]
    NoUser { uid: u32 },
    #[error("system bus: {0}")]
    Bus(#[from] BusError),
    /// UDisks refused to unmount because something still uses the volume.
    /// `message` is UDisks' own text (it usually names the holding process).
    #[error("{} is busy: {message}", mount_point.display())]
    Busy {
        mount_point: PathBuf,
        message: String,
    },
    /// polkit did not authorise the action (`NotAuthorized`,
    /// `NotAuthorizedCanObtain`) or the user closed its prompt
    /// (`NotAuthorizedDismissed`, `dismissed` true).
    #[error("not authorised: {message}")]
    NotAuthorized { message: String, dismissed: bool },
    /// UDisks knows no mounted filesystem at this path.
    #[error("{} is not a mount point UDisks manages", mount_point.display())]
    NotMounted { mount_point: PathBuf },
    /// The volume's drive is neither ejectable nor able to power off.
    /// Nothing was unmounted.
    #[error("the drive of {} cannot be ejected or powered off", mount_point.display())]
    NoEject { mount_point: PathBuf },
    /// Eject was refused before anything was unmounted: the drive could be
    /// a system disk.
    #[error("{} cannot be ejected: {reason}", mount_point.display())]
    EjectRefused {
        mount_point: PathBuf,
        reason: EjectRefusal,
    },
    /// An eject stopped part way. `unmounted` lists what is now unmounted and
    /// `locked` the encrypted blocks (UDisks object paths) now locked;
    /// `failed` is the mount point whose unmount failed, or the UDisks
    /// object path of the block whose lock or the drive whose eject or
    /// power-off failed. `cause` is the typed error (a dismissed polkit
    /// prompt, a busy volume, a timeout).
    #[error("eject incomplete: unmounted {} volume(s), locked {}, then {} failed: {cause}", unmounted.len(), locked.len(), failed.display())]
    EjectIncomplete {
        unmounted: Vec<PathBuf>,
        locked: Vec<PathBuf>,
        failed: PathBuf,
        #[source]
        cause: Box<VolumesError>,
    },
    /// No reply within the call's time limit. The operation may still
    /// complete; look at the mounts again before retrying.
    #[error("no reply from UDisks to {what}; it may still complete")]
    Timeout { what: String },
    /// Any other error reply from UDisks, with its D-Bus error name.
    #[error("UDisks {what} failed: {name}: {message}")]
    Udisks {
        what: String,
        name: String,
        message: String,
    },
    /// A transport failure or a reply that does not have the documented
    /// shape.
    #[error("UDisks {what}: {reason}")]
    Dbus { what: String, reason: String },
}

/// Why an eject was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EjectRefusal {
    #[error("the volume is not on a drive")]
    NoDrive,
    #[error("the drive is not removable")]
    NotRemovable,
    #[error("UDisks marks {device} on this drive as a system device")]
    SystemDevice { device: String },
    #[error("{device} on this drive is an LVM or RAID member, which cannot be taken apart safely")]
    Holder { device: String },
    #[error("{device} on this drive is {}", if *state_known { "active swap" } else { "swap whose state could not be read from Swapspace.Active" })]
    ActiveSwap { device: String, state_known: bool },
    #[error("{} is not a user mount", .0.display())]
    SystemMount(PathBuf),
}

impl VolumesError {
    pub(crate) fn io(path: &Path, source: std::io::Error) -> Self {
        VolumesError::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}
