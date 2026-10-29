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

use crate::identity::{disk_id, DiskId, Identity};
use crate::mountinfo::{current_user, parse_mountinfo_strict, read_mountinfo, Classifier};
use crate::{EjectRefusal, VolumesError};

/// How long a mount, unmount, eject or power-off may take, polkit prompt
/// included. Not a UDisks figure: long enough that a person answering a
/// password dialog or a drive flushing a large write is not cut off, short
/// enough that a hung `udisksd` ends in [`VolumesError::Timeout`] and not a
/// stuck caller.
pub const ACTION_TIMEOUT: Duration = Duration::from_secs(600);

pub(crate) const SERVICE: &str = "org.freedesktop.UDisks2";
pub(crate) const ROOT: &str = "/org/freedesktop/UDisks2";
pub(crate) const OBJECT_MANAGER: &str = "org.freedesktop.DBus.ObjectManager";
pub(crate) const BLOCK: &str = "org.freedesktop.UDisks2.Block";
pub(crate) const FILESYSTEM: &str = "org.freedesktop.UDisks2.Filesystem";
const DRIVE: &str = "org.freedesktop.UDisks2.Drive";
const SWAPSPACE: &str = "org.freedesktop.UDisks2.Swapspace";
const ENCRYPTED: &str = "org.freedesktop.UDisks2.Encrypted";
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
    /// `Block.IdUUID`, the filesystem UUID, empty when it has none. For
    /// display only: clones of a filesystem share it and some filesystems
    /// have none, so it is not checked.
    pub uuid: String,
    /// Which disk this is, from `Block.DeviceNumber` and the disk sequence
    /// number read when it was listed (see [`DiskId`] for what it is
    /// unique over). Mounting needs it to be known (see [`Identity::known`]).
    pub identity: Identity,
}

/// The device in `listed` that the device at `object_path` with identity
/// `wanted` stands for (the caller gets `wanted` from
/// [`Identity::known`] of the device it was given, so a device without one
/// never gets here). The device at `object_path` must be the same disk
/// (device number and sequence number) or it is
/// [`VolumesError::IdentityMismatch`], and one listed there without an
/// identity is [`VolumesError::IdentityUnknown`]; if nothing is at that path,
/// the device with the same identity under another path (the node was
/// renamed); else `None`.
pub fn find_listed(
    listed: Vec<Device>,
    object_path: &str,
    wanted: DiskId,
) -> Result<Option<Device>, VolumesError> {
    if let Some(at_path) = listed.iter().find(|d| d.object_path == object_path) {
        return match &at_path.identity {
            Identity::Known(id) if *id == wanted => Ok(Some(at_path.clone())),
            Identity::Known(id) => Err(VolumesError::IdentityMismatch {
                reason: format!(
                    "{object_path} is now device {}:{} with disk sequence number {}, not {}:{} \
                     with {}",
                    id.major, id.minor, id.diskseq, wanted.major, wanted.minor, wanted.diskseq
                ),
            }),
            Identity::Unknown { reason } => Err(VolumesError::IdentityUnknown {
                reason: reason.clone(),
            }),
        };
    }
    Ok(listed
        .into_iter()
        .find(|d| d.identity == Identity::Known(wanted)))
}

/// A connection to UDisks2.
pub struct Udisks {
    /// Reads, with the 3 s timeout.
    read: Connection,
    /// Mount, unmount, eject and power off, with [`ACTION_TIMEOUT`].
    act: Connection,
}

pub(crate) fn bad(what: &str, reason: impl Into<String>) -> VolumesError {
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
            uuid: get_string(ifaces, BLOCK, "IdUUID")?,
            identity: {
                let number = get_u64(ifaces, BLOCK, "DeviceNumber")?;
                Identity::from_result(disk_id(
                    rustix::fs::major(number),
                    rustix::fs::minor(number),
                ))
            },
        });
    }
    out.sort_by(|a, b| {
        a.device
            .cmp(&b.device)
            .then(a.object_path.cmp(&b.object_path))
    });
    Ok(out)
}

/// `Block.CryptoBackingDevice`: the encrypted block a cleartext block was
/// unlocked from (`/` or absent when it is not one).
fn backing_of(ifaces: &HashMap<String, Props>) -> Result<Option<String>, VolumesError> {
    match prop(ifaces, BLOCK, "CryptoBackingDevice") {
        None => Ok(None),
        Some(v) => {
            let owned = v
                .try_clone()
                .map_err(|e| bad("GetManagedObjects", e.to_string()))?;
            let p = OwnedObjectPath::try_from(owned)
                .map_err(|e| bad("GetManagedObjects", format!("CryptoBackingDevice: {e}")))?;
            Ok((p.as_str() != NO_OBJECT).then(|| p.to_string()))
        }
    }
}

/// Longest chain of stacked block devices followed (cleartext on encrypted
/// on partition is two links); a loop in the data ends the walk.
const MAX_STACK: usize = 16;

/// The drive a block belongs to, following `CryptoBackingDevice` for a
/// cleartext device (whose own `Block.Drive` is `/`).
fn resolve_drive(objects: &Objects, block: &str) -> Result<Option<String>, VolumesError> {
    let mut current = block.to_string();
    for _ in 0..MAX_STACK {
        let key = OwnedObjectPath::try_from(current.as_str())
            .map_err(|e| bad("GetManagedObjects", e.to_string()))?;
        let Some(ifaces) = objects.get(&key) else {
            return Ok(None);
        };
        if let Some(d) = drive_of(ifaces)? {
            return Ok(Some(d));
        }
        match backing_of(ifaces)? {
            Some(b) => current = b,
            None => return Ok(None),
        }
    }
    Ok(None)
}

/// The filesystem object mounted at `mount_point` (compared as paths, so a
/// trailing slash does not matter), with its drive object if any (found
/// through the encrypted block for an unlocked volume).
fn find_mounted(
    objects: &Objects,
    mount_point: &Path,
) -> Result<Option<(String, Option<String>)>, VolumesError> {
    for (path, ifaces) in objects {
        if !ifaces.contains_key(FILESYSTEM) {
            continue;
        }
        if mount_points(ifaces)?.iter().any(|m| m == mount_point) {
            let object = path.to_string();
            let drive = resolve_drive(objects, &object)?;
            return Ok(Some((object, drive)));
        }
    }
    Ok(None)
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
            act: kernkraft_bus::system_with_timeout(ACTION_TIMEOUT)?,
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

    /// Ejects the drive of the volume mounted at `mount_point`.
    ///
    /// The drive's members are its partitions and every block stacked on
    /// them through `CryptoBackingDevice` (an unlocked LUKS volume).
    /// Nothing is touched unless the drive is `Removable`, no member has
    /// `HintSystem`, no member is an LVM or RAID member, every mount on the
    /// drive is under `/run/media/<user>`, `/media` or `/mnt` and not on the
    /// system disk, and the drive is `Ejectable` or `CanPowerOff`; otherwise
    /// [`VolumesError::EjectRefused`] or [`VolumesError::NoEject`]. Then the
    /// volume and every other mounted filesystem on the drive are unmounted
    /// (an eject fails under mounted partitions), unlocked encrypted blocks
    /// are locked, and the drive is ejected if `Ejectable`, else powered off. A failure of the first unmount is its own error (a busy volume
    /// is [`VolumesError::Busy`]); a failure after something was unmounted is
    /// [`VolumesError::EjectIncomplete`] listing exactly what was.
    pub fn eject(&self, mount_point: &Path) -> Result<(), VolumesError> {
        let objects = self.objects()?;
        let (object, drive) =
            find_mounted(&objects, mount_point)?.ok_or_else(|| VolumesError::NotMounted {
                mount_point: mount_point.to_path_buf(),
            })?;
        let uid = rustix::process::getuid().as_raw();
        let user = current_user(uid, &mut Vec::new()).ok_or(VolumesError::NoUser { uid })?;
        let lines = parse_mountinfo_strict(read_mountinfo()?)?;
        let cls = Classifier::new(&lines, &user);
        // Only user mounts may be unmounted by an eject: under
        // /run/media/<user>, /media or /mnt, and not on the system disk.
        let may_unmount = |mp: &Path| {
            let dev = lines
                .iter()
                .rev()
                .find(|l| l.mount_point == mp)
                .map(|l| (l.major, l.minor));
            cls.in_user_trees(mp) && !cls.is_system(mp, dev)
        };
        let drive_info = match &drive {
            Some(d) => Some(drive_info(&objects, d)?),
            None => None,
        };
        let members = match &drive {
            Some(d) => members_of(&objects, d)?,
            None => Vec::new(),
        };
        let plan = plan_eject(
            &object,
            mount_point,
            drive_info.as_ref(),
            &members,
            &may_unmount,
        )?;
        let mut done = Done::default();
        for step in &plan.steps {
            let r = match step {
                Step::Unmount {
                    object,
                    mount_point,
                } => self.unmount_object(object, mount_point),
                Step::Lock { object } => self.act(object, ENCRYPTED, "Lock", None).map(drop),
            };
            if let Err(e) = r {
                if done.is_empty() {
                    return Err(e);
                }
                return Err(done.incomplete(step.target(), e));
            }
            done.record(step);
        }
        // A drive refusing here is not about one volume, so no mount point is
        // passed and a DeviceBusy stays a plain UDisks error.
        let drive = drive.unwrap_or_default();
        self.act(&drive, DRIVE, plan.method, None)
            .map(drop)
            .map_err(|e| done.incomplete(PathBuf::from(drive), e))
    }
}

/// What an eject has undone so far.
#[derive(Default)]
struct Done {
    unmounted: Vec<PathBuf>,
    locked: Vec<PathBuf>,
}

impl Done {
    fn is_empty(&self) -> bool {
        self.unmounted.is_empty() && self.locked.is_empty()
    }

    fn record(&mut self, step: &Step) {
        match step {
            Step::Unmount { mount_point, .. } => self.unmounted.push(mount_point.clone()),
            Step::Lock { object } => self.locked.push(PathBuf::from(object)),
        }
    }

    fn incomplete(self, failed: PathBuf, cause: VolumesError) -> VolumesError {
        VolumesError::EjectIncomplete {
            unmounted: self.unmounted,
            locked: self.locked,
            failed,
            cause: Box::new(cause),
        }
    }
}

/// The `Drive` properties eject decides on.
#[derive(Debug, Clone, Copy)]
struct DriveInfo {
    removable: bool,
    ejectable: bool,
    can_power_off: bool,
}

fn drive_info(objects: &Objects, drive: &str) -> Result<DriveInfo, VolumesError> {
    let empty = HashMap::new();
    let key = OwnedObjectPath::try_from(drive).map_err(|e| bad("eject", e.to_string()))?;
    let ifaces = objects.get(&key).unwrap_or(&empty);
    Ok(DriveInfo {
        removable: get_bool(ifaces, DRIVE, "Removable")?,
        ejectable: get_bool(ifaces, DRIVE, "Ejectable")?,
        can_power_off: get_bool(ifaces, DRIVE, "CanPowerOff")?,
    })
}

/// A block device on the drive being ejected, or stacked on one of its
/// partitions.
#[derive(Debug, Clone, Default)]
struct Member {
    object: String,
    device: String,
    mount_points: Vec<PathBuf>,
    system_hint: bool,
    /// The encrypted block this one is the cleartext of.
    backing: Option<String>,
    id_type: String,
    id_usage: String,
    /// `Swapspace.Active` of a swap member; `None` when it was not readable.
    swap_active: Option<bool>,
}

/// Every block whose drive, resolved through `CryptoBackingDevice`, is
/// `drive`, filesystem or not.
///
/// Only that stacking has a UDisks link. dm-integrity, bcache, loop devices
/// backed by files on the drive and multi-device btrfs have none, so they are
/// guarded only by `HintSystem` and the mount point checks.
fn members_of(objects: &Objects, drive: &str) -> Result<Vec<Member>, VolumesError> {
    let mut out = Vec::new();
    for (path, ifaces) in objects {
        if !ifaces.contains_key(BLOCK) {
            continue;
        }
        let object = path.to_string();
        if resolve_drive(objects, &object)?.as_deref() != Some(drive) {
            continue;
        }
        let device = match prop(ifaces, BLOCK, "Device") {
            Some(v) => String::from_utf8_lossy(&bytes_of(v, "Block.Device")?).into_owned(),
            None => String::new(),
        };
        out.push(Member {
            object,
            device,
            mount_points: mount_points(ifaces)?,
            system_hint: get_bool(ifaces, BLOCK, "HintSystem")?,
            backing: backing_of(ifaces)?,
            id_type: get_string(ifaces, BLOCK, "IdType")?,
            id_usage: get_string(ifaces, BLOCK, "IdUsage")?,
            swap_active: prop(ifaces, SWAPSPACE, "Active").and_then(|v| bool::try_from(v).ok()),
        });
    }
    out.sort_by(|a, b| a.object.cmp(&b.object));
    Ok(out)
}

/// One action of an eject.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    Unmount {
        object: String,
        mount_point: PathBuf,
    },
    /// `Encrypted.Lock` on the encrypted (backing) block.
    Lock { object: String },
}

impl Step {
    /// What `EjectIncomplete::failed` names when this step fails.
    fn target(&self) -> PathBuf {
        match self {
            Step::Unmount { mount_point, .. } => mount_point.clone(),
            Step::Lock { object } => PathBuf::from(object),
        }
    }
}

/// What an eject will do, decided before anything is touched.
#[derive(Debug, PartialEq, Eq)]
struct EjectPlan {
    /// Unmounts (the requested volume first, then the other mounted members)
    /// followed by the locks of their encrypted blocks.
    steps: Vec<Step>,
    /// `Eject` or `PowerOff`.
    method: &'static str,
}

fn plan_eject(
    requested: &str,
    mount_point: &Path,
    drive: Option<&DriveInfo>,
    members: &[Member],
    may_unmount: &dyn Fn(&Path) -> bool,
) -> Result<EjectPlan, VolumesError> {
    let refuse = |reason| VolumesError::EjectRefused {
        mount_point: mount_point.to_path_buf(),
        reason,
    };
    let Some(drive) = drive else {
        return Err(refuse(EjectRefusal::NoDrive));
    };
    if !drive.removable {
        return Err(refuse(EjectRefusal::NotRemovable));
    }
    if let Some(m) = members.iter().find(|m| m.system_hint) {
        return Err(refuse(EjectRefusal::SystemDevice {
            device: m.device.clone(),
        }));
    }
    // Active swap has no mount point, so the mount checks cannot see it, and
    // powering off or locking under it would fail or cut off live memory.
    // Swap whose state cannot be read is refused as well.
    if let Some(m) = members
        .iter()
        .find(|m| m.id_type == "swap" && m.swap_active != Some(false))
    {
        return Err(refuse(EjectRefusal::ActiveSwap {
            device: m.device.clone(),
            state_known: m.swap_active.is_some(),
        }));
    }
    // UDisks does not expose what an LVM physical volume or a RAID member
    // holds (logical volumes, array devices) well enough to unmount and
    // deactivate them safely, so a drive carrying one is never ejected.
    if let Some(m) = members.iter().find(|m| {
        m.id_type == "LVM2_member" || m.id_type == "linux_raid_member" || m.id_usage == "raid"
    }) {
        return Err(refuse(EjectRefusal::Holder {
            device: m.device.clone(),
        }));
    }
    if let Some(mp) = members
        .iter()
        .flat_map(|m| &m.mount_points)
        .find(|mp| !may_unmount(mp))
    {
        return Err(refuse(EjectRefusal::SystemMount(mp.clone())));
    }
    let method = if drive.ejectable {
        "Eject"
    } else if drive.can_power_off {
        "PowerOff"
    } else {
        return Err(VolumesError::NoEject {
            mount_point: mount_point.to_path_buf(),
        });
    };
    let mut steps: Vec<Step> = Vec::new();
    let ordered = members
        .iter()
        .filter(|m| m.object == requested)
        .chain(members.iter().filter(|m| m.object != requested));
    for m in ordered {
        if let Some(p) = m.mount_points.first() {
            steps.push(Step::Unmount {
                object: m.object.clone(),
                mount_point: p.clone(),
            });
        }
    }
    for m in members {
        if let Some(b) = &m.backing {
            let lock = Step::Lock { object: b.clone() };
            if !steps.contains(&lock) {
                steps.push(lock);
            }
        }
    }
    Ok(EjectPlan { steps, method })
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

    fn with_system_hint(mut o: HashMap<String, Props>) -> HashMap<String, Props> {
        o.get_mut(BLOCK)
            .expect("block")
            .insert("HintSystem".to_string(), val(true));
        o
    }

    #[test]
    fn unmounted_lists_system_device_without_drive() {
        let mut o = Objects::new();
        o.insert(
            OwnedObjectPath::try_from("/o/nvme").expect("path"),
            with_system_hint(fs_object("/dev/nvme0n1p9", false, &[], "/")),
        );
        let devs = unmounted_devices(&o).expect("ok");
        assert_eq!(devs.len(), 1);
        assert!(devs[0].system);
        assert_eq!(drive_of(o.values().next().expect("one")).expect("ok"), None);
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

    fn with_device_number(
        mut o: HashMap<String, Props>,
        major: u32,
        minor: u32,
    ) -> HashMap<String, Props> {
        // dev_t as UDisks reports `Block.DeviceNumber` (glibc encoding).
        let dev_t = (u64::from(major & 0xfff) << 8)
            | u64::from(minor & 0xff)
            | (u64::from(minor & !0xff) << 12)
            | (u64::from(major & !0xfff) << 32);
        o.get_mut(BLOCK)
            .expect("block")
            .insert("DeviceNumber".to_string(), val(dev_t));
        o
    }

    fn device(object_path: &str, identity: Identity) -> Device {
        Device {
            object_path: object_path.to_string(),
            device: String::new(),
            label: String::new(),
            fs_type: String::new(),
            size: 0,
            system: false,
            uuid: String::new(),
            identity,
        }
    }

    fn known(major: u32, minor: u32, diskseq: u64) -> Identity {
        Identity::Known(crate::DiskId {
            major,
            minor,
            diskseq,
        })
    }

    /// The device number decodes (glibc encoding, including the extended
    /// fields) and, with no such block device in sysfs, the identity says so.
    #[test]
    fn unmounted_identity_comes_from_the_device_number() {
        let mut o = Objects::new();
        o.insert(
            OwnedObjectPath::try_from("/o/nvme").expect("path"),
            with_device_number(fs_object("/dev/x", false, &[], "/"), 4095, 300),
        );
        let devs = unmounted_devices(&o).expect("ok");
        match &devs[0].identity {
            Identity::Unknown { reason } => {
                assert!(reason.contains("4095:300"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
        // The decoding itself.
        let dev_t = (u64::from(259u32 & 0xfff) << 8)
            | u64::from(300u32 & 0xff)
            | (u64::from(300u32 & !0xff) << 12);
        assert_eq!(
            (rustix::fs::major(dev_t), rustix::fs::minor(dev_t)),
            (259, 300)
        );
    }

    #[test]
    fn listed_device_is_matched_by_path_then_checked_by_identity() {
        let listed = vec![
            device("/o/sdb1", known(8, 17, 5)),
            device("/o/sdc1", known(8, 33, 6)),
        ];
        let id = |major, minor, diskseq| DiskId {
            major,
            minor,
            diskseq,
        };
        // Same path, same disk.
        let got = find_listed(listed.clone(), "/o/sdb1", id(8, 17, 5)).expect("ok");
        assert_eq!(got.expect("found").object_path, "/o/sdb1");
        // Same path, another disk now (the next sequence number): refused.
        assert!(matches!(
            find_listed(listed.clone(), "/o/sdb1", id(8, 17, 7)),
            Err(VolumesError::IdentityMismatch { .. })
        ));
        // The path is gone but the disk is listed under another path.
        let got = find_listed(listed.clone(), "/o/sdx9", id(8, 33, 6)).expect("ok");
        assert_eq!(got.expect("found").object_path, "/o/sdc1");
        // Not listed at all.
        assert!(find_listed(listed, "/o/sdx9", id(9, 9, 9))
            .expect("ok")
            .is_none());
        // A listed device without an identity is not passed over silently.
        let unknown = Identity::Unknown { reason: "x".into() };
        let listed = vec![device("/o/sdb1", unknown)];
        assert!(matches!(
            find_listed(listed, "/o/sdb1", id(8, 17, 5)),
            Err(VolumesError::IdentityUnknown { .. })
        ));
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

    fn member(object: &str, device: &str, mounts: &[&str], system: bool) -> Member {
        Member {
            object: object.to_string(),
            device: device.to_string(),
            mount_points: mounts.iter().map(PathBuf::from).collect(),
            system_hint: system,
            ..Member::default()
        }
    }

    fn typed(mut m: Member, id_type: &str) -> Member {
        m.id_type = id_type.to_string();
        m
    }

    fn cleartext(object: &str, backing: &str, mounts: &[&str]) -> Member {
        Member {
            backing: Some(backing.to_string()),
            ..member(object, "/dev/dm-0", mounts, false)
        }
    }

    const STICK: DriveInfo = DriveInfo {
        removable: true,
        ejectable: false,
        can_power_off: true,
    };

    /// `bad` lists mount points that fail the system test; otherwise only
    /// the three user trees may be unmounted.
    fn plan(
        drive: Option<&DriveInfo>,
        members: &[Member],
        bad: &[&str],
    ) -> Result<EjectPlan, VolumesError> {
        plan_for("/o/b", drive, members, bad)
    }

    fn plan_for(
        requested: &str,
        drive: Option<&DriveInfo>,
        members: &[Member],
        bad: &[&str],
    ) -> Result<EjectPlan, VolumesError> {
        plan_eject(
            requested,
            Path::new("/run/media/u/B"),
            drive,
            members,
            &|p| {
                let in_trees = ["/run/media/u", "/media", "/mnt"]
                    .iter()
                    .any(|t| p.starts_with(t));
                in_trees && !bad.iter().any(|s| p == Path::new(s))
            },
        )
    }

    fn refusal(r: Result<EjectPlan, VolumesError>) -> EjectRefusal {
        match r {
            Err(VolumesError::EjectRefused { reason, .. }) => reason,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn plan_unmounts_requested_first_then_siblings_and_powers_off() {
        let members = [
            member("/o/a", "/dev/sdb1", &["/run/media/u/A"], false),
            member("/o/b", "/dev/sdb2", &["/run/media/u/B"], false),
            member("/o/c", "/dev/sdb3", &[], false),
        ];
        let p = plan(Some(&STICK), &members, &[]).expect("allowed");
        assert_eq!(
            p.steps,
            vec![
                Step::Unmount {
                    object: "/o/b".into(),
                    mount_point: PathBuf::from("/run/media/u/B")
                },
                Step::Unmount {
                    object: "/o/a".into(),
                    mount_point: PathBuf::from("/run/media/u/A")
                },
            ]
        );
        assert_eq!(p.method, "PowerOff");
        let ej = DriveInfo {
            ejectable: true,
            ..STICK
        };
        assert_eq!(plan(Some(&ej), &members, &[]).expect("ok").method, "Eject");
    }

    #[test]
    fn plan_refuses_system_drives_before_unmounting() {
        let usb = member("/o/b", "/dev/sdb2", &["/run/media/u/B"], false);
        assert_eq!(
            refusal(plan(None, std::slice::from_ref(&usb), &[])),
            EjectRefusal::NoDrive
        );
        let fixed = DriveInfo {
            removable: false,
            ..STICK
        };
        assert_eq!(
            refusal(plan(Some(&fixed), std::slice::from_ref(&usb), &[])),
            EjectRefusal::NotRemovable
        );
        // A system hint on any block device of the drive, mounted or not.
        let hinted = member("/o/z", "/dev/sdb9", &[], true);
        assert_eq!(
            refusal(plan(Some(&STICK), &[usb.clone(), hinted], &[])),
            EjectRefusal::SystemDevice {
                device: "/dev/sdb9".into()
            }
        );
        // A sibling mounted at a system place.
        let home = member("/o/h", "/dev/sdb3", &["/home"], false);
        assert_eq!(
            refusal(plan(Some(&STICK), &[usb.clone(), home], &["/home"])),
            EjectRefusal::SystemMount(PathBuf::from("/home"))
        );
        // Neither ejectable nor able to power off: nothing is unmounted.
        let dead = DriveInfo {
            can_power_off: false,
            ..STICK
        };
        assert!(matches!(
            plan(Some(&dead), std::slice::from_ref(&usb), &[]),
            Err(VolumesError::NoEject { .. })
        ));
    }

    #[test]
    fn plan_refuses_a_sibling_outside_the_user_trees() {
        let usb = member("/o/b", "/dev/sdb2", &["/run/media/u/B"], false);
        for odd in ["/var", "/usr", "/opt", "/srv/data"] {
            let sib = member("/o/v", "/dev/sdb3", &[odd], false);
            assert_eq!(
                refusal(plan(Some(&STICK), &[usb.clone(), sib], &[])),
                EjectRefusal::SystemMount(PathBuf::from(odd))
            );
        }
    }

    #[test]
    fn luks_stick_eject_plan_unmounts_locks_ejects() {
        let members = [
            typed(member("/o/p1", "/dev/sdb1", &[], false), "crypto_LUKS"),
            cleartext("/o/dm", "/o/p1", &["/run/media/u/B"]),
        ];
        let p = plan_for("/o/dm", Some(&STICK), &members, &[]).expect("allowed");
        assert_eq!(
            p.steps,
            vec![
                Step::Unmount {
                    object: "/o/dm".into(),
                    mount_point: PathBuf::from("/run/media/u/B")
                },
                Step::Lock {
                    object: "/o/p1".into()
                },
            ]
        );
        assert_eq!(p.method, "PowerOff");
    }

    #[test]
    fn luks_root_on_usb_refuses_sibling_eject() {
        let members = [
            typed(member("/o/p1", "/dev/sdb1", &[], false), "crypto_LUKS"),
            cleartext("/o/root", "/o/p1", &["/"]),
            member("/o/b", "/dev/sdb2", &["/run/media/u/B"], false),
        ];
        assert_eq!(
            refusal(plan(Some(&STICK), &members, &[])),
            EjectRefusal::SystemMount(PathBuf::from("/"))
        );
    }

    #[test]
    fn lvm_member_refuses() {
        let usb = member("/o/b", "/dev/sdb2", &["/run/media/u/B"], false);
        for kind in ["LVM2_member", "linux_raid_member"] {
            let pv = typed(member("/o/pv", "/dev/sdb3", &[], false), kind);
            assert_eq!(
                refusal(plan(Some(&STICK), &[usb.clone(), pv], &[])),
                EjectRefusal::Holder {
                    device: "/dev/sdb3".into()
                }
            );
        }
        let mut raid = member("/o/r", "/dev/sdb4", &[], false);
        raid.id_usage = "raid".into();
        assert!(matches!(
            refusal(plan(Some(&STICK), &[usb, raid], &[])),
            EjectRefusal::Holder { .. }
        ));
    }

    #[test]
    fn stacked_blocks_resolve_to_the_drive_of_their_backing_device() {
        let key = |p: &str| OwnedObjectPath::try_from(p).expect("path");
        let mut o = objects();
        // A cleartext block on /o/sdb1: its own Drive is "/", the backing one is the stick.
        let mut dm = fs_object("/dev/dm-0", false, &[b"/run/media/u/L"], "/");
        dm.get_mut(BLOCK)
            .expect("block")
            .insert("CryptoBackingDevice".into(), val(key("/o/sdb1")));
        o.insert(key("/o/dm0"), dm);
        let (obj, drive) = find_mounted(&o, Path::new("/run/media/u/L"))
            .expect("ok")
            .expect("found");
        assert_eq!(
            (obj.as_str(), drive.as_deref()),
            ("/o/dm0", Some("/o/drive"))
        );
        let members = members_of(&o, "/o/drive").expect("ok");
        let names: Vec<_> = members.iter().map(|m| m.object.as_str()).collect();
        assert_eq!(names, vec!["/o/dm0", "/o/sdb1", "/o/sdb2"]);
        assert_eq!(members[0].backing.as_deref(), Some("/o/sdb1"));
        // A backing loop ends instead of spinning.
        let mut a = fs_object("/dev/a", false, &[], "/");
        a.get_mut(BLOCK)
            .expect("block")
            .insert("CryptoBackingDevice".into(), val(key("/o/a")));
        o.insert(key("/o/a"), a);
        assert_eq!(resolve_drive(&o, "/o/a").expect("ok"), None);
    }

    #[test]
    fn active_swap_member_refuses() {
        let usb = member("/o/b", "/dev/sdb2", &["/run/media/u/B"], false);
        let swap = |active: Option<bool>| Member {
            swap_active: active,
            ..typed(member("/o/s", "/dev/sdb3", &[], false), "swap")
        };
        assert_eq!(
            refusal(plan(Some(&STICK), &[usb.clone(), swap(Some(true))], &[])),
            EjectRefusal::ActiveSwap {
                device: "/dev/sdb3".into(),
                state_known: true
            }
        );
        // Unreadable state is refused too.
        assert_eq!(
            refusal(plan(Some(&STICK), &[usb.clone(), swap(None)], &[])),
            EjectRefusal::ActiveSwap {
                device: "/dev/sdb3".into(),
                state_known: false
            }
        );
        // A stacked swap (cleartext of a LUKS partition) counts.
        let stacked = Member {
            swap_active: Some(true),
            id_type: "swap".into(),
            ..cleartext("/o/sw", "/o/p1", &[])
        };
        assert!(matches!(
            refusal(plan(Some(&STICK), &[usb.clone(), stacked], &[])),
            EjectRefusal::ActiveSwap { .. }
        ));
        // An inactive swap partition does not refuse.
        assert!(plan(Some(&STICK), &[usb, swap(Some(false))], &[]).is_ok());
    }
}
