// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Volumes for the Places panel: what is mounted (proc(5),
//! `/proc/self/mountinfo`), what could be (UDisks2 D-Bus API), and mount,
//! unmount and eject through UDisks2.
//!
//! Mount, unmount and eject are plain commands, run on Tauri's blocking
//! pool like every other, whose promise settles when UDisks answers; they
//! are not background operations in the registry. They can take long (a
//! polkit prompt waits on the user, up to `kernkraft_volumes::ACTION_TIMEOUT`),
//! but there is nothing to report on the way and nothing to cancel: UDisks
//! offers no way to cancel a call in flight, and the user ends a prompt by
//! dismissing it, which the command reports as `NotAuthorized`. Nor do they
//! touch files the undo journal knows, so they have no reason to make undo
//! wait. The UI shows the volume as busy until the promise settles.

use std::path::PathBuf;

use kernkraft_volumes::{list, Device, ListProblem, Mount, Udisks};
use serde::{Deserialize, Serialize};
use tauri::Runtime;

use crate::error::{PluginError, WireError};
use crate::state::Kernkraft;
use crate::wire::WirePath;

/// A mounted volume. `mount_point` is the exact path; the rest are display
/// strings (see `kernkraft_volumes::Mount`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireMount {
    pub mount_point: WirePath,
    pub device: String,
    pub fs_type: String,
    pub label: String,
    pub removable: bool,
    pub read_only: bool,
}

impl From<&Mount> for WireMount {
    fn from(m: &Mount) -> WireMount {
        WireMount {
            mount_point: WirePath::from(&m.mount_point),
            device: m.device.clone(),
            fs_type: m.fs_type.clone(),
            label: m.label.clone(),
            removable: m.removable,
            read_only: m.read_only,
        }
    }
}

/// The Places panel's volumes: mounted ones, filesystems UDisks could
/// mount, and what could not be found out. A problem with one source
/// leaves the other's volumes listed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireVolumes {
    pub mounted: Vec<WireMount>,
    pub unmounted: Vec<Device>,
    pub problems: Vec<WireError>,
}

/// A listing problem as a [`WireError`] whose `kind` is the
/// `ListProblem` variant's name.
fn problem(p: &ListProblem) -> WireError {
    let (kind, message) = match p {
        ListProblem::MalformedLine { line, reason } => (
            "MalformedLine",
            format!("mountinfo line {line} skipped, so a volume may be missing: {reason}"),
        ),
        ListProblem::Labels(e) => (
            "Labels",
            format!("volume labels could not be read; folder names are shown: {e}"),
        ),
        ListProblem::Passwd(e) => ("Passwd", format!("/etc/passwd could not be read: {e}")),
        ListProblem::NoUserName { uid } => (
            "NoUserName",
            format!("no user name for uid {uid}, so volumes under /run/media are missing"),
        ),
    };
    WireError {
        kind: kind.to_owned(),
        message,
        path: None,
        details: None,
    }
}

impl<R: Runtime> Kernkraft<R> {
    /// The mounted volumes, and the filesystems UDisks lists as unmounted.
    /// Only failing to read `/proc/self/mountinfo` fails the call; UDisks or
    /// the system bus being unavailable is a problem in the listing, with
    /// `unmounted` empty.
    pub fn volumes_list(&self) -> Result<WireVolumes, WireError> {
        let listing = list()?;
        let mut problems: Vec<WireError> = listing.problems.iter().map(problem).collect();
        let unmounted = match Udisks::connect().and_then(|u| u.unmounted()) {
            Ok(devices) => devices,
            Err(e) => {
                problems.push(WireError::from(e));
                Vec::new()
            }
        };
        Ok(WireVolumes {
            mounted: listing.mounts.iter().map(WireMount::from).collect(),
            unmounted,
            problems,
        })
    }

    /// Mounts `device`, which must be one UDisks lists as unmounted now
    /// (matched by its object path; the webview's copy is not trusted), and
    /// returns where UDisks mounted it. `UnknownDevice` otherwise.
    pub fn volume_mount(&self, device: Device) -> Result<WirePath, WireError> {
        let udisks = Udisks::connect()?;
        let listed = udisks
            .unmounted()?
            .into_iter()
            .find(|d| d.object_path == device.object_path)
            .ok_or_else(|| PluginError::UnknownDevice(device.object_path.clone()))?;
        Ok(WirePath::from(&udisks.mount(&listed)?))
    }

    /// Unmounts the filesystem at `mount_point`. `Busy` (with UDisks'
    /// message) while something uses it.
    pub fn volume_unmount(&self, mount_point: WirePath) -> Result<(), WireError> {
        let mount_point = PathBuf::try_from(mount_point)?;
        Ok(Udisks::connect()?.unmount(&mount_point)?)
    }

    /// Unmounts every volume on the drive of `mount_point` and ejects or
    /// powers off the drive (see `kernkraft_volumes::Udisks::eject` for
    /// what is refused). A failure after something was unmounted is
    /// `EjectIncomplete`, whose details list what was.
    pub fn volume_eject(&self, mount_point: WirePath) -> Result<(), WireError> {
        let mount_point = PathBuf::try_from(mount_point)?;
        Ok(Udisks::connect()?.eject(&mount_point)?)
    }
}
