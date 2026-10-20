// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Errors on the IPC wire.

use std::path::Path;

use kernkraft_bus::BusError;
use kernkraft_fs::FsError;
use kernkraft_theme::ThemeError;
use kernkraft_thumb::ThumbError;
use kernkraft_volumes::VolumesError;
use kernkraft_xdg_apps::AppsError;
use serde::{Deserialize, Serialize};

use crate::wire::{WirePath, WirePathError};

/// A failure as the webview receives it.
///
/// `kind` is the name of the error variant (`ExternalModification`,
/// `Exists`, `NotFound`, `Changed`, `Irreversible`, ...), so the UI branches
/// on it without parsing `message`. `path` is the path the failure is about,
/// when the variant names one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireError {
    pub kind: String,
    pub message: String,
    pub path: Option<WirePath>,
}

impl WireError {
    fn new(kind: &str, message: String, path: Option<&Path>) -> WireError {
        WireError {
            kind: kind.to_owned(),
            message,
            path: path.map(WirePath::from),
        }
    }
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

impl std::error::Error for WireError {}

impl From<WirePathError> for WireError {
    fn from(e: WirePathError) -> WireError {
        let kind = match e {
            WirePathError::BadBase64(_) => "BadBase64",
            WirePathError::Nul => "Nul",
        };
        WireError::new(kind, e.to_string(), None)
    }
}

impl From<&FsError> for WireError {
    fn from(e: &FsError) -> WireError {
        let (kind, path) = match e {
            FsError::Io { path, .. } => ("Io", Some(path)),
            FsError::ExternalModification { path } => ("ExternalModification", Some(path)),
            FsError::Exists { path } => ("Exists", Some(path)),
            FsError::NotFound { path } => ("NotFound", Some(path)),
            FsError::InvalidName { .. } => ("InvalidName", None),
            FsError::IntoItself { src } => ("IntoItself", Some(src)),
            FsError::TrashUnavailable { path } => ("TrashUnavailable", Some(path)),
            FsError::BadTrashInfo { path } => ("BadTrashInfo", Some(path)),
            FsError::UnsafeCacheDir { path } => ("UnsafeCacheDir", Some(path)),
            FsError::OwnershipNotKept { path } => ("OwnershipNotKept", Some(path)),
            FsError::XattrsNotKept { path, .. } => ("XattrsNotKept", Some(path)),
            FsError::Cancelled => ("Cancelled", None),
            FsError::Changed { path, .. } => ("Changed", Some(path)),
            FsError::Irreversible { .. } => ("Irreversible", None),
            FsError::Partial { .. } => ("Partial", None),
            FsError::SourceNotRemoved { left, .. } => ("SourceNotRemoved", Some(left)),
            FsError::NothingToUndo => ("NothingToUndo", None),
            FsError::NothingToRedo => ("NothingToRedo", None),
        };
        WireError::new(kind, e.to_string(), path.map(|p| p.as_path()))
    }
}

impl From<FsError> for WireError {
    fn from(e: FsError) -> WireError {
        WireError::from(&e)
    }
}

impl From<kernkraft_text::TextError> for WireError {
    fn from(e: kernkraft_text::TextError) -> WireError {
        use kernkraft_text::TextError;
        match e {
            TextError::Fs(inner) => WireError::from(inner),
            TextError::TooLargeToOpen { .. } => {
                WireError::new("TooLargeToOpen", e.to_string(), None)
            }
        }
    }
}

impl From<AppsError> for WireError {
    fn from(e: AppsError) -> WireError {
        let (kind, path) = match &e {
            AppsError::Io { path, .. } => ("Io", Some(path)),
            AppsError::MalformedMimeDb { path, .. } => ("MalformedMimeDb", Some(path)),
            AppsError::InvalidDesktopEntry { path, .. } => ("InvalidDesktopEntry", Some(path)),
            AppsError::NotAnApplication { path, .. } => ("NotAnApplication", Some(path)),
            AppsError::MalformedMimeApps { path, .. } => ("MalformedMimeApps", Some(path)),
            AppsError::InvalidExec { .. } => ("InvalidExec", None),
            AppsError::NoFileArgument { .. } => ("NoFileArgument", None),
            AppsError::Launch { .. } => ("Launch", None),
            AppsError::XdgMime { .. } => ("XdgMime", None),
            AppsError::NoTerminal { .. } => ("NoTerminal", None),
            AppsError::InvalidArgument { .. } => ("InvalidArgument", None),
        };
        WireError::new(kind, e.to_string(), path.map(|p| p.as_path()))
    }
}

impl From<ThumbError> for WireError {
    fn from(e: ThumbError) -> WireError {
        let (kind, path) = match &e {
            ThumbError::Io { path, .. } => ("Io", Some(path)),
            ThumbError::Decode { path, .. } => ("Decode", Some(path)),
            ThumbError::Unsupported { .. } => ("Unsupported", None),
            ThumbError::Fs(inner) => return WireError::from(inner),
        };
        WireError::new(kind, e.to_string(), path.map(|p| p.as_path()))
    }
}

impl From<ThemeError> for WireError {
    fn from(e: ThemeError) -> WireError {
        let (kind, path) = match &e {
            ThemeError::Io { path, .. } => ("Io", Some(path)),
            ThemeError::TooLarge { path, .. } => ("TooLarge", Some(path)),
            ThemeError::Portal(_) => ("Portal", None),
            ThemeError::Watch { path, .. } => ("Watch", Some(path)),
            ThemeError::CallbackPanic(_) => ("CallbackPanic", None),
        };
        WireError::new(kind, e.to_string(), path.map(|p| p.as_path()))
    }
}

impl From<VolumesError> for WireError {
    fn from(e: VolumesError) -> WireError {
        let (kind, path) = match &e {
            VolumesError::Io { path, .. } => ("Io", Some(path)),
            VolumesError::MalformedMountinfo { .. } => ("MalformedMountinfo", None),
            VolumesError::NoUser { .. } => ("NoUser", None),
            VolumesError::Bus(_) => ("Bus", None),
            VolumesError::Busy { mount_point, .. } => ("Busy", Some(mount_point)),
            VolumesError::NotAuthorized { .. } => ("NotAuthorized", None),
            VolumesError::NotMounted { mount_point } => ("NotMounted", Some(mount_point)),
            VolumesError::NoEject { mount_point } => ("NoEject", Some(mount_point)),
            VolumesError::EjectRefused { mount_point, .. } => ("EjectRefused", Some(mount_point)),
            VolumesError::EjectIncomplete { failed, .. } => ("EjectIncomplete", Some(failed)),
            VolumesError::Timeout { .. } => ("Timeout", None),
            VolumesError::Udisks { .. } => ("Udisks", None),
            VolumesError::Dbus { .. } => ("Dbus", None),
        };
        WireError::new(kind, e.to_string(), path.map(|p| p.as_path()))
    }
}

impl From<BusError> for WireError {
    fn from(e: BusError) -> WireError {
        WireError::from(&e)
    }
}

impl From<&BusError> for WireError {
    fn from(e: &BusError) -> WireError {
        let kind = match e {
            BusError::NoAddress(_) => "NoAddress",
            BusError::NotUnix(_) => "NotUnix",
            BusError::Malformed { .. } => "Malformed",
            BusError::Connect { .. } => "Connect",
            BusError::Handshake { .. } => "Handshake",
        };
        WireError::new(kind, e.to_string(), None)
    }
}
