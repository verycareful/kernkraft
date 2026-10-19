// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Mount, unmount and eject through UDisks2 on the system bus.
//!
//! Specification: UDisks2 D-Bus API reference
//! (<https://storaged.org/doc/udisks2-api/latest/>): the
//! `org.freedesktop.UDisks2` service; `GetManagedObjects` of
//! `org.freedesktop.DBus.ObjectManager` at `/org/freedesktop/UDisks2`; the
//! `Block` (`Device`, `Drive`, `IdLabel`, `IdType`, `Size`, `HintIgnore`,
//! `HintSystem`), `Filesystem` (`MountPoints`, `Mount`, `Unmount`) and `Drive`
//! (`Ejectable`, `CanPowerOff`, `Eject`, `PowerOff`) interfaces; the
//! `org.freedesktop.UDisks2.Error.*` names; and "Standard Options" (an empty
//! `a{sv}` lets polkit prompt the user, `auth.no_user_interaction` would stop
//! it).
//!
//! Calls that only read (`GetManagedObjects`) use the 3 s connection of
//! `kernkraft-bus`. zbus sets its method timeout on the connection, not per
//! call, so `Mount`, `Unmount`, `Eject` and `PowerOff` (a slow stick, a
//! polkit prompt the user has not answered yet) go over a second connection
//! with [`ACTION_TIMEOUT`].

use std::collections::HashMap;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use zbus::blocking::Connection;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::DBusError;

use crate::VolumesError;

/// How long a mount, unmount, eject or power-off may take, polkit prompt
/// included. Not a UDisks figure: long enough that a person answering a
/// password dialog or a drive flushing a large write is not cut off, short
/// enough that a hung `udisksd` ends in [`VolumesError::Timeout`] and not a
/// stuck caller.
pub const ACTION_TIMEOUT: Duration = Duration::from_secs(600);

const SERVICE: &str = "org.freedesktop.UDisks2";
const ROOT: &str = "/org/freedesktop/UDisks2";
const OBJECT_MANAGER: &str = "org.freedesktop.DBus.ObjectManager";
const BLOCK: &str = "org.freedesktop.UDisks2.Block";
const FILESYSTEM: &str = "org.freedesktop.UDisks2.Filesystem";
const DRIVE: &str = "org.freedesktop.UDisks2.Drive";
const ERROR_PREFIX: &str = "org.freedesktop.UDisks2.Error.";
/// The object path UDisks puts in `Block.Drive` when there is no drive.
const NO_OBJECT: &str = "/";

type Props = HashMap<String, OwnedValue>;
type Objects = HashMap<OwnedObjectPath, HashMap<String, Props>>;

/// A filesystem UDisks knows that is not mounted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    /// The UDisks object, the handle [`Udisks::mount`] uses.
    pub object_path: String,
    /// Device node, such as `/dev/sdb1`.
    pub device: String,
    /// `Block.IdLabel`, empty when the filesystem has none.
    pub label: String,
    /// `Block.IdType`, such as `ext4`.
    pub fs_type: String,
    /// Size in bytes.
    pub size: u64,
    /// `Block.HintSystem`: UDisks thinks this is a fixed system disk, not
    /// removable media. Listed all the same; the caller decides what to show.
    pub system: bool,
}

/// A connection to UDisks2.
pub struct Udisks {
    /// Reads, with the 3 s timeout.
    read: Connection,
    /// Mount, unmount, eject and power off, with [`ACTION_TIMEOUT`].
    act: Connection,
}

fn bad(what: &str, reason: impl Into<String>) -> VolumesError {
    VolumesError::Dbus {
        what: what.to_string(),
        reason: reason.into(),
    }
}

fn prop<'a>(ifaces: &'a HashMap<String, Props>, iface: &str, name: &str) -> Option<&'a OwnedValue> {
    ifaces.get(iface)?.get(name)
}

fn get_bool(
    ifaces: &HashMap<String, Props>,
    iface: &str,
    name: &str,
) -> Result<bool, VolumesError> {
    match prop(ifaces, iface, name) {
        None => Ok(false),
        Some(v) => {
            bool::try_from(v).map_err(|e| bad("GetManagedObjects", format!("{iface}.{name}: {e}")))
        }
    }
}

fn get_u64(ifaces: &HashMap<String, Props>, iface: &str, name: &str) -> Result<u64, VolumesError> {
    match prop(ifaces, iface, name) {
        None => Ok(0),
        Some(v) => {
            u64::try_from(v).map_err(|e| bad("GetManagedObjects", format!("{iface}.{name}: {e}")))
        }
    }
}

fn get_string(
    ifaces: &HashMap<String, Props>,
    iface: &str,
    name: &str,
) -> Result<String, VolumesError> {
    match prop(ifaces, iface, name) {
        None => Ok(String::new()),
        Some(v) => <&str>::try_from(v)
            .map(str::to_string)
            .map_err(|e| bad("GetManagedObjects", format!("{iface}.{name}: {e}"))),
    }
}

/// A byte-string property (`ay`) without its trailing NUL.
fn bytes_of(v: &OwnedValue, what: &str) -> Result<Vec<u8>, VolumesError> {
    let owned = v
        .try_clone()
        .map_err(|e| bad("GetManagedObjects", format!("{what}: {e}")))?;
    let mut bytes =
        Vec::<u8>::try_from(owned).map_err(|e| bad("GetManagedObjects", format!("{what}: {e}")))?;
    if bytes.last() == Some(&0) {
        bytes.pop();
    }
    Ok(bytes)
}

/// `Filesystem.MountPoints` (`aay`, each NUL-terminated).
fn mount_points(ifaces: &HashMap<String, Props>) -> Result<Vec<PathBuf>, VolumesError> {
    let Some(v) = prop(ifaces, FILESYSTEM, "MountPoints") else {
        return Ok(Vec::new());
    };
    let owned = v
        .try_clone()
        .map_err(|e| bad("GetManagedObjects", format!("MountPoints: {e}")))?;
    let rows = Vec::<Vec<u8>>::try_from(owned)
        .map_err(|e| bad("GetManagedObjects", format!("MountPoints: {e}")))?;
    Ok(rows
        .into_iter()
        .map(|mut b| {
            if b.last() == Some(&0) {
                b.pop();
            }
            PathBuf::from(OsString::from_vec(b))
        })
        .collect())
}

fn drive_of(ifaces: &HashMap<String, Props>) -> Result<Option<String>, VolumesError> {
    match prop(ifaces, BLOCK, "Drive") {
        None => Ok(None),
        Some(v) => {
            let p = OwnedObjectPath::try_from(
                v.try_clone()
                    .map_err(|e| bad("GetManagedObjects", e.to_string()))?,
            )
            .map_err(|e| bad("GetManagedObjects", format!("Block.Drive: {e}")))?;
            Ok((p.as_str() != NO_OBJECT).then(|| p.to_string()))
        }
    }
}

/// The filesystems in `objects` that are not ignored and not mounted.
fn unmounted_devices(objects: &Objects) -> Result<Vec<Device>, VolumesError> {
    let mut out = Vec::new();
    for (path, ifaces) in objects {
        if !ifaces.contains_key(FILESYSTEM) || !ifaces.contains_key(BLOCK) {
            continue;
        }
        if get_bool(ifaces, BLOCK, "HintIgnore")? || !mount_points(ifaces)?.is_empty() {
            continue;
        }
        let device = match prop(ifaces, BLOCK, "Device") {
            Some(v) => String::from_utf8_lossy(&bytes_of(v, "Block.Device")?).into_owned(),
            None => String::new(),
        };
        out.push(Device {
            object_path: path.to_string(),
            device,
            label: get_string(ifaces, BLOCK, "IdLabel")?,
            fs_type: get_string(ifaces, BLOCK, "IdType")?,
            size: get_u64(ifaces, BLOCK, "Size")?,
            system: get_bool(ifaces, BLOCK, "HintSystem")?,
        });
    }
    out.sort_by(|a, b| {
        a.device
            .cmp(&b.device)
            .then(a.object_path.cmp(&b.object_path))
    });
    Ok(out)
}

/// The filesystem object mounted at `mount_point` (compared as paths, so a
/// trailing slash does not matter), with its drive object if any.
fn find_mounted(
    objects: &Objects,
    mount_point: &Path,
) -> Result<Option<(String, Option<String>)>, VolumesError> {
    for (path, ifaces) in objects {
        if !ifaces.contains_key(FILESYSTEM) {
            continue;
        }
        if mount_points(ifaces)?.iter().any(|m| m == mount_point) {
            return Ok(Some((path.to_string(), drive_of(ifaces)?)));
        }
    }
    Ok(None)
}

/// Every mounted filesystem on `drive`, as (object path, first mount point),
/// in object path order.
fn mounted_on_drive(
    objects: &Objects,
    drive: &str,
) -> Result<Vec<(String, PathBuf)>, VolumesError> {
    let mut out = Vec::new();
    for (path, ifaces) in objects {
        if !ifaces.contains_key(FILESYSTEM) || drive_of(ifaces)?.as_deref() != Some(drive) {
            continue;
        }
        if let Some(first) = mount_points(ifaces)?.into_iter().next() {
            out.push((path.to_string(), first));
        }
    }
    out.sort();
    Ok(out)
}

/// Maps a UDisks error name to the crate's errors. `mount_point` is the
/// volume the call concerns, if any.
fn classify(what: &str, name: &str, message: String, mount_point: Option<&Path>) -> VolumesError {
    let short = name.strip_prefix(ERROR_PREFIX);
    match (short, mount_point) {
        (Some("DeviceBusy"), Some(mp)) => VolumesError::Busy {
            mount_point: mp.to_path_buf(),
            message,
        },
        (Some("NotAuthorized" | "NotAuthorizedCanObtain"), _) => VolumesError::NotAuthorized {
            message,
            dismissed: false,
        },
        (Some("NotAuthorizedDismissed"), _) => VolumesError::NotAuthorized {
            message,
            dismissed: true,
        },
        (Some("NotMounted"), Some(mp)) => VolumesError::NotMounted {
            mount_point: mp.to_path_buf(),
        },
        _ => VolumesError::Udisks {
            what: what.to_string(),
            name: name.to_string(),
            message,
        },
    }
}

fn call_error(what: &str, e: zbus::Error, mount_point: Option<&Path>) -> VolumesError {
    match e {
        zbus::Error::MethodError(name, desc, _) => {
            classify(what, name.as_str(), desc.unwrap_or_default(), mount_point)
        }
        zbus::Error::FDO(f) => classify(what, f.name().as_str(), f.to_string(), mount_point),
        zbus::Error::InputOutput(io) if io.kind() == std::io::ErrorKind::TimedOut => {
            VolumesError::Timeout {
                what: what.to_string(),
            }
        }
        other => bad(what, other.to_string()),
    }
}

impl Udisks {
    /// Connects to the system bus (twice, see the module doc) over its Unix
    /// socket. UDisks being absent shows on the first call that needs it.
    pub fn connect() -> Result<Udisks, VolumesError> {
        Ok(Udisks {
            read: kernkraft_bus::system()?,
            act: kernkraft_bus::system_with_timeout(Some(ACTION_TIMEOUT))?,
        })
    }

    fn objects(&self) -> Result<Objects, VolumesError> {
        let what = "GetManagedObjects";
        let msg = self
            .read
            .call_method(Some(SERVICE), ROOT, Some(OBJECT_MANAGER), what, &())
            .map_err(|e| call_error(what, e, None))?;
        let body = msg.body();
        body.deserialize::<Objects>()
            .map_err(|e| bad(what, format!("reply: {e}")))
    }

    /// Calls `method` on an object over the long-timeout connection with
    /// empty options (so polkit may prompt) and returns the reply.
    fn act(
        &self,
        path: &str,
        iface: &str,
        method: &str,
        mount_point: Option<&Path>,
    ) -> Result<zbus::Message, VolumesError> {
        let options: HashMap<&str, Value<'_>> = HashMap::new();
        self.act
            .call_method(Some(SERVICE), path, Some(iface), method, &options)
            .map_err(|e| call_error(method, e, mount_point))
    }

    /// Filesystems UDisks lists (`HintIgnore` false) that have no mount point.
    pub fn unmounted(&self) -> Result<Vec<Device>, VolumesError> {
        unmounted_devices(&self.objects()?)
    }

    /// Mounts `dev` and returns where UDisks put it. UDisks decides the
    /// directory (under `/run/media/<user>`); polkit may ask the user.
    pub fn mount(&self, dev: &Device) -> Result<PathBuf, VolumesError> {
        let reply = self.act(&dev.object_path, FILESYSTEM, "Mount", None)?;
        let body = reply.body();
        let path: String = body
            .deserialize()
            .map_err(|e| bad("Mount", format!("reply: {e}")))?;
        Ok(PathBuf::from(path))
    }

    /// Unmounts the filesystem mounted at `mount_point`. A volume something
    /// still uses is [`VolumesError::Busy`], carrying UDisks' message.
    pub fn unmount(&self, mount_point: &Path) -> Result<(), VolumesError> {
        let objects = self.objects()?;
        let (object, _) =
            find_mounted(&objects, mount_point)?.ok_or_else(|| VolumesError::NotMounted {
                mount_point: mount_point.to_path_buf(),
            })?;
        self.unmount_object(&object, mount_point)
    }

    fn unmount_object(&self, object: &str, mount_point: &Path) -> Result<(), VolumesError> {
        self.act(object, FILESYSTEM, "Unmount", Some(mount_point))
            .map(drop)
    }

    /// Unmounts the volume at `mount_point` and every other filesystem on its
    /// drive (a power-off or eject would fail under mounted partitions), then
    /// ejects the drive if it is `Ejectable`, else powers it off if it
    /// `CanPowerOff`. A drive that can do neither, or a volume with no drive,
    /// is left unmounted and reported as [`VolumesError::NoEject`].
    pub fn eject(&self, mount_point: &Path) -> Result<(), VolumesError> {
        let objects = self.objects()?;
        let (object, drive) =
            find_mounted(&objects, mount_point)?.ok_or_else(|| VolumesError::NotMounted {
                mount_point: mount_point.to_path_buf(),
            })?;
        let Some(drive) = drive else {
            self.unmount_object(&object, mount_point)?;
            return Err(VolumesError::NoEject {
                mount_point: mount_point.to_path_buf(),
            });
        };
        self.unmount_object(&object, mount_point)?;
        for (other, other_point) in mounted_on_drive(&objects, &drive)? {
            if other != object {
                self.unmount_object(&other, &other_point)?;
            }
        }
        let empty = HashMap::new();
        let flags = objects
            .get(
                &OwnedObjectPath::try_from(drive.as_str())
                    .map_err(|e| bad("eject", e.to_string()))?,
            )
            .unwrap_or(&empty);
        let method = if get_bool(flags, DRIVE, "Ejectable")? {
            "Eject"
        } else if get_bool(flags, DRIVE, "CanPowerOff")? {
            "PowerOff"
        } else {
            return Err(VolumesError::NoEject {
                mount_point: mount_point.to_path_buf(),
            });
        };
        self.act(&drive, DRIVE, method, Some(mount_point)).map(drop)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    fn val<T: Into<Value<'static>>>(v: T) -> OwnedValue {
        OwnedValue::try_from(v.into()).expect("plain values convert")
    }

    fn props(items: Vec<(&str, OwnedValue)>) -> Props {
        items.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
    }

    fn fs_object(
        device: &str,
        ignore: bool,
        mounts: &[&[u8]],
        drive: &str,
    ) -> HashMap<String, Props> {
        let rows: Vec<Vec<u8>> = mounts.iter().map(|m| [m, &[0u8][..]].concat()).collect();
        let mut dev = device.as_bytes().to_vec();
        dev.push(0);
        HashMap::from([
            (
                BLOCK.to_string(),
                props(vec![
                    ("Device", val(dev)),
                    ("HintIgnore", val(ignore)),
                    ("IdLabel", val("DATA")),
                    (
                        "Drive",
                        val(OwnedObjectPath::try_from(drive).unwrap_or_default()),
                    ),
                ]),
            ),
            (
                FILESYSTEM.to_string(),
                props(vec![("MountPoints", val(rows))]),
            ),
        ])
    }

    fn objects() -> Objects {
        let key = |p: &str| OwnedObjectPath::try_from(p).expect("valid object path");
        HashMap::from([
            (
                key("/o/sdb1"),
                fs_object("/dev/sdb1", false, &[b"/run/media/u/A B"], "/o/drive"),
            ),
            (
                key("/o/sdb2"),
                fs_object("/dev/sdb2", false, &[], "/o/drive"),
            ),
            (key("/o/sdc1"), fs_object("/dev/sdc1", true, &[], "/")),
        ])
    }

    #[test]
    fn unmounted_skips_ignored_and_mounted() {
        let devs = unmounted_devices(&objects()).expect("well-formed");
        assert_eq!(devs.len(), 1);
        assert_eq!(devs[0].device, "/dev/sdb2");
        assert_eq!(devs[0].label, "DATA");
    }

    #[test]
    fn finds_mount_point_bytes_and_drive() {
        let o = objects();
        let found = find_mounted(&o, Path::new("/run/media/u/A B")).expect("ok");
        assert_eq!(
            found,
            Some(("/o/sdb1".to_string(), Some("/o/drive".to_string())))
        );
        assert_eq!(
            find_mounted(&o, Path::new("/run/media/u/other")).expect("ok"),
            None
        );
        let non_utf8 = Path::new(std::ffi::OsStr::from_bytes(b"/mnt/\xff"));
        let mut o2 = Objects::new();
        o2.insert(
            OwnedObjectPath::try_from("/o/x").expect("path"),
            fs_object("/dev/x", false, &[b"/mnt/\xff"], "/"),
        );
        assert_eq!(
            find_mounted(&o2, non_utf8).expect("ok"),
            Some(("/o/x".to_string(), None))
        );
    }

    #[test]
    fn mounted_on_drive_lists_siblings() {
        let m = mounted_on_drive(&objects(), "/o/drive").expect("ok");
        assert_eq!(
            m,
            vec![("/o/sdb1".to_string(), PathBuf::from("/run/media/u/A B"))]
        );
    }

    #[test]
    fn error_names_map_to_variants() {
        let mp = Path::new("/run/media/u/X");
        let n = |s: &str| format!("{ERROR_PREFIX}{s}");
        assert!(matches!(
            classify("Unmount", &n("DeviceBusy"), "target is busy".into(), Some(mp)),
            VolumesError::Busy { message, .. } if message == "target is busy"
        ));
        assert!(matches!(
            classify("Mount", &n("NotAuthorizedCanObtain"), "m".into(), None),
            VolumesError::NotAuthorized {
                dismissed: false,
                ..
            }
        ));
        assert!(matches!(
            classify("Mount", &n("NotAuthorized"), "m".into(), None),
            VolumesError::NotAuthorized {
                dismissed: false,
                ..
            }
        ));
        assert!(matches!(
            classify("Mount", &n("NotAuthorizedDismissed"), "m".into(), None),
            VolumesError::NotAuthorized {
                dismissed: true,
                ..
            }
        ));
        assert!(matches!(
            classify("Unmount", &n("NotMounted"), "m".into(), Some(mp)),
            VolumesError::NotMounted { .. }
        ));
        assert!(matches!(
            classify("Mount", &n("AlreadyMounted"), "m".into(), None),
            VolumesError::Udisks { .. }
        ));
    }
}
