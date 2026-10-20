// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Errors on the IPC wire.

use std::path::{Path, PathBuf};

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
/// when the variant names one. `details` carries the structured parts of the
/// variants that hold more than one path or a cause (see [`WireDetails`]),
/// so the UI can show every item of them, not only the message.
///
/// Kind policy for errors that wrap another error: a variant that only
/// forwards another crate's error (`TextError::Fs`, `ThumbError::Fs`,
/// `VolumesError::Bus`) reports the inner error's `kind`, `path` and
/// `details`, with the outer error's `message`. Every other variant reports
/// its own name as `kind`, including variants that carry a cause beside
/// their own data (`Partial`, `SourceNotRemoved`, `EjectIncomplete`); their
/// cause is in `details`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireError {
    pub kind: String,
    pub message: String,
    pub path: Option<WirePath>,
    /// Boxed: most errors have none, and the box keeps `WireError` small
    /// in every `Result`.
    #[serde(default)]
    pub details: Option<Box<WireDetails>>,
}

/// One path with the error it met: an item of a report or of a
/// [`WireDetails::Partial`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireItemError {
    pub path: WirePath,
    pub error: WireError,
}

impl WireItemError {
    pub(crate) fn new(path: &Path, error: &FsError) -> WireItemError {
        WireItemError {
            path: WirePath::from(path),
            error: WireError::from(error),
        }
    }
}

/// The structured parts of an error, for the variants that have any.
/// JSON: `{"k":"Partial","done":[...],...}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "k")]
pub enum WireDetails {
    /// [`FsError::Partial`]: an undo or redo that stopped partway.
    Partial {
        done: Vec<WirePath>,
        not_done: Vec<WirePath>,
        cause: Box<WireError>,
        warnings: Vec<WireItemError>,
    },
    /// [`FsError::SourceNotRemoved`]: both `copy` and `left` now exist.
    SourceNotRemoved {
        copy: WirePath,
        left: WirePath,
        cause: Box<WireError>,
    },
    /// [`FsError::XattrsNotKept`]: the attribute names not carried over.
    XattrsNotKept { names: Vec<String> },
    /// [`FsError::Changed`]: what the undo found, as the name of the
    /// `Change` variant (`Modified`, `Missing`, `Taken`, `Unverified`).
    Changed { what: String },
    /// [`FsError::Irreversible`]: why, as the name of the
    /// `IrreversibleReason` variant.
    Irreversible { reason: String },
    /// [`VolumesError::EjectIncomplete`]: what the eject did before `failed`
    /// stopped it. `failed` may be a UDisks object path, not a file path.
    EjectIncomplete {
        unmounted: Vec<WirePath>,
        locked: Vec<WirePath>,
        failed: WirePath,
        cause: Box<WireError>,
    },
}

impl WireError {
    fn new(kind: &str, message: String, path: Option<&Path>) -> WireError {
        WireError {
            kind: kind.to_owned(),
            message,
            path: path.map(WirePath::from),
            details: None,
        }
    }

    fn with_details(mut self, details: WireDetails) -> WireError {
        self.details = Some(Box::new(details));
        self
    }

    /// `inner` as the error of a variant that only forwards it: its kind,
    /// path and details, with `message`.
    fn forwarded(inner: WireError, message: String) -> WireError {
        WireError { message, ..inner }
    }
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

impl std::error::Error for WireError {}

fn wire_paths(paths: &[PathBuf]) -> Vec<WirePath> {
    paths.iter().map(WirePath::from).collect()
}

/// Failures of the plugin itself, as opposed to the crates it calls.
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    /// Undo or redo was asked for while a file operation runs, or an
    /// operation was asked for while an undo or redo runs.
    #[error("busy: {0}")]
    Busy(&'static str),
    #[error("no running operation {0}")]
    UnknownOp(u64),
    #[error("operation {op} has no open conflict {conflict}")]
    UnknownConflict { op: u64, conflict: u64 },
    #[error("no folder is watched under {0:?}")]
    UnknownWatch(String),
    #[error("invalid {what}: {value:?}")]
    InvalidArgument { what: &'static str, value: String },
    /// `HOME` is unset, empty or relative, so the XDG base directories, and
    /// the trash with them, are unknown.
    #[error("HOME is not set to an absolute path; the trash and data directories are unknown")]
    NoHome,
    /// A trash item the webview named is not in the trash as listed now.
    #[error("{} is not in the trash", .0.display())]
    NotInTrash(PathBuf),
    #[error("watching {}: {source}", path.display())]
    Watch {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// A worker panicked or could not be joined; a bug, reported instead of
    /// lost.
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<PluginError> for WireError {
    fn from(e: PluginError) -> WireError {
        let (kind, path) = match &e {
            PluginError::Busy(_) => ("Busy", None),
            PluginError::UnknownOp(_) => ("UnknownOp", None),
            PluginError::UnknownConflict { .. } => ("UnknownConflict", None),
            PluginError::UnknownWatch(_) => ("UnknownWatch", None),
            PluginError::InvalidArgument { .. } => ("InvalidArgument", None),
            PluginError::NoHome => ("NoHome", None),
            PluginError::NotInTrash(path) => ("NotInTrash", Some(path)),
            PluginError::Watch { path, .. } => ("Watch", Some(path)),
            PluginError::Internal(_) => ("Internal", None),
        };
        WireError::new(kind, e.to_string(), path.map(|p| p.as_path()))
    }
}

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
        let wire = WireError::new(kind, e.to_string(), path.map(|p| p.as_path()));
        let details = match e {
            FsError::Partial {
                done,
                not_done,
                cause,
                warnings,
            } => WireDetails::Partial {
                done: wire_paths(done),
                not_done: wire_paths(not_done),
                cause: Box::new(WireError::from(cause.as_ref())),
                warnings: warnings
                    .iter()
                    .map(|(p, w)| WireItemError::new(p, w))
                    .collect(),
            },
            FsError::SourceNotRemoved { copy, left, cause } => WireDetails::SourceNotRemoved {
                copy: WirePath::from(copy),
                left: WirePath::from(left),
                cause: Box::new(WireError::from(cause.as_ref())),
            },
            FsError::XattrsNotKept { names, .. } => WireDetails::XattrsNotKept {
                names: names.clone(),
            },
            FsError::Changed { what, .. } => WireDetails::Changed {
                what: format!("{what:?}"),
            },
            FsError::Irreversible { reason, .. } => WireDetails::Irreversible {
                reason: format!("{reason:?}"),
            },
            _ => return wire,
        };
        wire.with_details(details)
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
        match &e {
            TextError::Fs(inner) => WireError::forwarded(WireError::from(inner), e.to_string()),
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
            ThumbError::Fs(inner) => {
                return WireError::forwarded(WireError::from(inner), e.to_string())
            }
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
        WireError::from(&e)
    }
}

impl From<&VolumesError> for WireError {
    fn from(e: &VolumesError) -> WireError {
        let (kind, path) = match e {
            VolumesError::Io { path, .. } => ("Io", Some(path)),
            VolumesError::MalformedMountinfo { .. } => ("MalformedMountinfo", None),
            VolumesError::NoUser { .. } => ("NoUser", None),
            VolumesError::Bus(inner) => {
                return WireError::forwarded(WireError::from(inner), e.to_string())
            }
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
        let wire = WireError::new(kind, e.to_string(), path.map(|p| p.as_path()));
        match e {
            VolumesError::EjectIncomplete {
                unmounted,
                locked,
                failed,
                cause,
            } => wire.with_details(WireDetails::EjectIncomplete {
                unmounted: wire_paths(unmounted),
                locked: wire_paths(locked),
                failed: WirePath::from(failed),
                cause: Box::new(WireError::from(cause.as_ref())),
            }),
            _ => wire,
        }
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
