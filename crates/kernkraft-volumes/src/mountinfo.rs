// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! `/proc/self/mountinfo` and the mounts a user cares about.
//!
//! Format: proc(5), "/proc/pid/mountinfo". One mount per line:
//! `id parent major:minor root mount-point mount-options [optional-fields...] - fstype source super-options`.
//! The number of optional fields varies and only the lone `-` ends them.
//! The kernel writes space, tab, newline and backslash inside a field as a
//! three-digit octal escape (`\040`, `\011`, `\012`, `\134`), and nothing
//! else is escaped, so a field may still hold bytes that are not UTF-8.
//!
//! Volume labels come from udev's `/dev/disk/by-label` links; udev writes
//! unsafe bytes in their names as `\xNN` (udev `util_replace_chars` and
//! `udev_util_encode_string`).

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use crate::VolumesError;
use serde::{Deserialize, Serialize};

const MOUNTINFO: &str = "/proc/self/mountinfo";
const BY_LABEL: &str = "/dev/disk/by-label";
const PASSWD: &str = "/etc/passwd";

/// Filesystems that hold no user files: kernel interfaces, memory-backed
/// stores, container layers, compressed system images and automount stubs.
/// `fuse.portal` is the xdg-desktop-portal document store.
const PSEUDO_FS: &[&str] = &[
    "proc",
    "sysfs",
    "tmpfs",
    "devtmpfs",
    "cgroup",
    "cgroup2",
    "overlay",
    "squashfs",
    "autofs",
    "securityfs",
    "pstore",
    "bpf",
    "tracefs",
    "debugfs",
    "configfs",
    "fusectl",
    "mqueue",
    "hugetlbfs",
    "efivarfs",
    "ramfs",
    "binfmt_misc",
    "nsfs",
    "devpts",
    "fuse.portal",
];

/// One parsed mountinfo line. `root`, `mount_point` and the option fields
/// are exact; `source` and the option strings are ASCII in practice and are
/// converted lossily.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountInfoLine {
    pub mount_id: u32,
    pub parent_id: u32,
    pub major: u32,
    pub minor: u32,
    pub root: PathBuf,
    pub mount_point: PathBuf,
    pub mount_options: Vec<String>,
    pub optional_fields: Vec<String>,
    pub fs_type: String,
    pub source: String,
    pub super_options: Vec<String>,
}

/// A volume as the Places panel shows it. `mount_point` is the exact path;
/// `device`, `fs_type` and `label` are display strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    /// Mountinfo field 1, the mount ID: unique among the mounts of the
    /// mount namespace while the mount exists, and reused after it is
    /// unmounted. With `major` and `minor` it identifies one mounted
    /// filesystem instance (see [`MountKey`]).
    pub mount_id: u32,
    /// Mountinfo field 3, `major:minor` of the filesystem's device
    /// (`st_dev` of its files). Several mounts of one filesystem (a bind
    /// mount, btrfs subvolumes) share it; the mount ID tells them apart.
    pub major: u32,
    pub minor: u32,
    pub mount_point: PathBuf,
    pub device: String,
    pub fs_type: String,
    pub label: String,
    /// Mounted under `/run/media/<user>` or `/media`, where UDisks and the
    /// desktop put removable media.
    pub removable: bool,
    /// `ro` in the mount options or the superblock options.
    pub read_only: bool,
}

/// What names one mounted filesystem instance: the mount ID and device
/// number from its mountinfo line. The mount ID alone is reused once a mount
/// is gone and the device number alone is shared by bind mounts; a different
/// disk mounted at the same place gets a different pair in practice, which is
/// what [`resolve_mount`] checks. Neither value survives a reboot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MountKey {
    pub mount_id: u32,
    pub major: u32,
    pub minor: u32,
}

impl Mount {
    pub fn key(&self) -> MountKey {
        MountKey {
            mount_id: self.mount_id,
            major: self.major,
            minor: self.minor,
        }
    }
}

/// The mount point to act on, from the mount point the caller knows, the
/// identity it was listed with, or both: [`resolve_mount_in`] on the current
/// `/proc/self/mountinfo`.
pub fn resolve_mount(
    mount_point: Option<&Path>,
    key: Option<MountKey>,
) -> Result<PathBuf, VolumesError> {
    let lines = parse_mountinfo_strict(read_mountinfo()?)?;
    resolve_mount_in(&lines, mount_point, key)
}

/// The pure half of [`resolve_mount`].
///
/// With a `key`, the mount with that ID must exist, be on that device, and
/// be at `mount_point` when one is given; the result is where it is now.
/// Anything else is [`VolumesError::IdentityMismatch`], because the volume
/// the caller listed is gone or replaced. With only a `mount_point`, that
/// path is returned as it is: it is the caller's word and is checked by what
/// acts on it. With neither, there is nothing to resolve.
pub fn resolve_mount_in(
    lines: &[MountInfoLine],
    mount_point: Option<&Path>,
    key: Option<MountKey>,
) -> Result<PathBuf, VolumesError> {
    let mismatch = |reason: String| VolumesError::IdentityMismatch { reason };
    let Some(key) = key else {
        return mount_point
            .map(Path::to_path_buf)
            .ok_or_else(|| mismatch("neither a mount point nor an identity was given".into()));
    };
    let line = lines
        .iter()
        .rev()
        .find(|l| l.mount_id == key.mount_id)
        .ok_or_else(|| mismatch(format!("mount ID {} no longer exists", key.mount_id)))?;
    if (line.major, line.minor) != (key.major, key.minor) {
        return Err(mismatch(format!(
            "mount ID {} is now device {}:{}, not {}:{}",
            key.mount_id, line.major, line.minor, key.major, key.minor
        )));
    }
    if let Some(mp) = mount_point.filter(|mp| *mp != line.mount_point) {
        return Err(mismatch(format!(
            "mount ID {} is at {}, not {}",
            key.mount_id,
            line.mount_point.display(),
            mp.display()
        )));
    }
    Ok(line.mount_point.clone())
}

/// Decodes the `\ooo` octal escapes of one mountinfo field. A backslash not
/// followed by exactly three octal digits that fit a byte is an error.
fn unescape_octal(field: &[u8]) -> Result<Vec<u8>, &'static str> {
    let mut out = Vec::with_capacity(field.len());
    let mut i = 0;
    while i < field.len() {
        if field[i] != b'\\' {
            out.push(field[i]);
            i += 1;
            continue;
        }
        let digits = field.get(i + 1..i + 4).ok_or("truncated octal escape")?;
        if !digits.iter().all(|d| (b'0'..=b'7').contains(d)) {
            return Err("bad octal escape");
        }
        let v = digits
            .iter()
            .fold(0u32, |acc, d| acc * 8 + u32::from(d - b'0'));
        out.push(u8::try_from(v).map_err(|_| "octal escape above 255")?);
        i += 4;
    }
    Ok(out)
}

fn text(field: &[u8]) -> String {
    String::from_utf8_lossy(field).into_owned()
}

fn number(field: &[u8], reason: &'static str) -> Result<u32, &'static str> {
    std::str::from_utf8(field)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or(reason)
}

fn csv(field: &[u8]) -> Vec<String> {
    field
        .split(|&b| b == b',')
        .filter(|o| !o.is_empty())
        .map(text)
        .collect()
}

fn parse_line(line: &[u8]) -> Result<MountInfoLine, &'static str> {
    let tokens: Vec<&[u8]> = line.split(|&b| b == b' ').collect();
    // Six fixed fields come first; the separator can only follow them.
    let sep = tokens
        .iter()
        .skip(6)
        .position(|t| *t == b"-")
        .map(|p| p + 6)
        .ok_or("no - separator after the fixed fields")?;
    let [fs_type, source, super_options] = tokens[sep + 1..] else {
        return Err("expected filesystem type, source and super options after -");
    };
    let (major, minor) = {
        let (a, b) = tokens[2]
            .iter()
            .position(|&c| c == b':')
            .map(|p| (&tokens[2][..p], &tokens[2][p + 1..]))
            .ok_or("device number is not major:minor")?;
        (
            number(a, "bad major number")?,
            number(b, "bad minor number")?,
        )
    };
    if tokens[3].is_empty() || tokens[4].is_empty() || fs_type.is_empty() {
        return Err("empty root, mount point or filesystem type");
    }
    let path = |f: &[u8]| -> Result<PathBuf, &'static str> {
        Ok(PathBuf::from(OsString::from_vec(unescape_octal(f)?)))
    };
    Ok(MountInfoLine {
        mount_id: number(tokens[0], "bad mount id")?,
        parent_id: number(tokens[1], "bad parent id")?,
        major,
        minor,
        root: path(tokens[3])?,
        mount_point: path(tokens[4])?,
        mount_options: csv(tokens[5]),
        optional_fields: tokens[6..sep].iter().map(|t| text(t)).collect(),
        fs_type: text(fs_type),
        source: text(&unescape_octal(source)?),
        super_options: csv(super_options),
    })
}

/// Each non-blank line with its 1-based number and parse result.
fn parse_each(
    contents: &[u8],
) -> impl Iterator<Item = (usize, Result<MountInfoLine, &'static str>)> + '_ {
    contents
        .split(|&b| b == b'\n')
        .enumerate()
        .filter(|(_, l)| !l.is_empty())
        .map(|(i, l)| (i + 1, parse_line(l)))
}

/// Parses mountinfo text. A malformed line is skipped, never a panic; use
/// [`parse_mountinfo_report`] or [`parse_mountinfo_strict`] when a skipped
/// line must not go unnoticed.
pub fn parse_mountinfo(contents: &str) -> Vec<MountInfoLine> {
    parse_mountinfo_report(contents).0
}

/// Parses mountinfo text (or the raw bytes of the file, which may hold
/// non-UTF-8 names), skipping malformed lines and returning each as a
/// [`ListProblem::MalformedLine`].
pub fn parse_mountinfo_report(
    contents: impl AsRef<[u8]>,
) -> (Vec<MountInfoLine>, Vec<ListProblem>) {
    let mut lines = Vec::new();
    let mut problems = Vec::new();
    for (line, r) in parse_each(contents.as_ref()) {
        match r {
            Ok(l) => lines.push(l),
            Err(reason) => problems.push(ListProblem::MalformedLine { line, reason }),
        }
    }
    (lines, problems)
}

/// Like [`parse_mountinfo_report`] but fails on the first malformed line,
/// reporting its number.
pub fn parse_mountinfo_strict(
    contents: impl AsRef<[u8]>,
) -> Result<Vec<MountInfoLine>, VolumesError> {
    parse_each(contents.as_ref())
        .map(|(line, r)| r.map_err(|reason| VolumesError::MalformedMountinfo { line, reason }))
        .collect()
}

/// The directory UDisks mounts removable media of `user` in.
fn media_dir(user: &str) -> Option<PathBuf> {
    // A name that is empty or could climb out of /run/media is no user name.
    if user.is_empty() || user.contains('/') || user == "." || user == ".." {
        return None;
    }
    Some(Path::new("/run/media").join(user))
}

fn has_option(options: &[String], name: &str) -> bool {
    options.iter().any(|o| o == name)
}

/// Network and remote filesystems. Probing one for write access goes through
/// the mount to a server that may be unreachable, so outside the named
/// trees they are neither probed nor listed. The names are the filesystem
/// types the kernel registers for remote storage (NFS, the CIFS/SMB client,
/// 9P, Ceph, AFS) and the FUSE subtypes mountinfo shows as `fuse.<name>` or
/// bare names of the same services (SSHFS, GlusterFS, davfs2).
const REMOTE_FS: &[&str] = &[
    "nfs",
    "nfs4",
    "cifs",
    "smb3",
    "smbfs",
    "9p",
    "ceph",
    "glusterfs",
    "fuse.sshfs",
    "davfs",
    "afs",
];

type DevNum = (u32, u32);

/// Where volumes are expected and which mounts belong to the system.
pub(crate) struct Classifier {
    media: Option<PathBuf>,
    /// Device numbers of the `/` and `/home` mounts (the last line for each,
    /// since a later mount covers an earlier one at the same point).
    system_devices: Vec<DevNum>,
}

impl Classifier {
    pub(crate) fn new(lines: &[MountInfoLine], user: &str) -> Classifier {
        let last = |p: &str| {
            lines
                .iter()
                .rev()
                .find(|l| l.mount_point == Path::new(p))
                .map(|l| (l.major, l.minor))
        };
        Classifier {
            media: media_dir(user),
            system_devices: [last("/"), last("/home")].into_iter().flatten().collect(),
        }
    }

    /// Under `/run/media/<user>`, `/media` or `/mnt`: where mounts a person
    /// made or UDisks made for them live.
    pub(crate) fn in_user_trees(&self, p: &Path) -> bool {
        self.media.as_deref().is_some_and(|m| p.starts_with(m))
            || p.starts_with("/media")
            || p.starts_with("/mnt")
    }

    /// A mount that belongs to the system and is never a volume to show or to
    /// unmount: the places `/`, `/home`, `/boot*`, `/efi`, the kernel trees,
    /// the rest of `/run`, and, outside the user trees, anything on the
    /// device of `/` or `/home` (subvolumes and bind mounts of the system
    /// disk). With no `/` or `/home` line nothing is excluded by device.
    pub(crate) fn is_system(&self, p: &Path, dev: Option<DevNum>) -> bool {
        if p == Path::new("/")
            || p == Path::new("/home")
            || p.as_os_str().as_bytes().starts_with(b"/boot")
            || p.starts_with("/efi")
            || p.starts_with("/proc")
            || p.starts_with("/sys")
            || p.starts_with("/dev")
        {
            return true;
        }
        if p.starts_with("/run") {
            return !self.in_user_trees(p);
        }
        !self.in_user_trees(p) && dev.is_some_and(|d| self.system_devices.contains(&d))
    }
}

/// The mounts of `lines` that a person would call volumes.
///
/// Kept: everything under `/run/media/<user>`, `/media` and `/mnt`, and any
/// other mount `is_writable` accepts. Dropped, whatever `is_writable` says:
/// pseudo filesystems, `/`, `/boot*`, `/efi`, `/home` itself, anything under
/// `/proc`, `/sys` or `/dev`, anything under `/run` other than the user's
/// media directory, and outside the three trees any mount on the device of
/// `/` or `/home` (a subvolume or bind mount of the system disk).
///
/// Outside the three trees two more rules apply: remote filesystems
/// (NFS, CIFS and the like) are dropped without calling `is_writable`, and a
/// FUSE mount whose `user_id=` option names another uid than `uid` belongs
/// to someone else and is dropped. Inside the trees the owner is not
/// checked: ntfs-3g mounts a user's stick as `fuseblk` with `user_id=0`.
///
/// `label` is the mount point's last component; [`list`] replaces it with
/// the filesystem label when udev has one.
pub fn user_mounts(
    lines: &[MountInfoLine],
    uid: u32,
    user: &str,
    is_writable: &dyn Fn(&Path) -> bool,
) -> Vec<Mount> {
    let cls = Classifier::new(lines, user);
    lines
        .iter()
        .filter(|l| {
            let p = l.mount_point.as_path();
            if PSEUDO_FS.contains(&l.fs_type.as_str()) || cls.is_system(p, Some((l.major, l.minor)))
            {
                return false;
            }
            if cls.in_user_trees(p) {
                return true;
            }
            if REMOTE_FS.contains(&l.fs_type.as_str()) {
                return false;
            }
            if l.fs_type.starts_with("fuse") {
                let owner = l
                    .super_options
                    .iter()
                    .find_map(|o| o.strip_prefix("user_id="));
                if owner.is_some_and(|o| o.parse() != Ok(uid)) {
                    return false;
                }
            }
            is_writable(p)
        })
        .map(|l| Mount {
            mount_id: l.mount_id,
            major: l.major,
            minor: l.minor,
            mount_point: l.mount_point.clone(),
            device: l.source.clone(),
            fs_type: l.fs_type.clone(),
            label: base_name(&l.mount_point),
            removable: cls
                .media
                .as_deref()
                .is_some_and(|m| l.mount_point.starts_with(m))
                || l.mount_point.starts_with("/media"),
            read_only: has_option(&l.mount_options, "ro") || has_option(&l.super_options, "ro"),
        })
        .collect()
}

fn base_name(p: &Path) -> String {
    p.file_name()
        .unwrap_or(p.as_os_str())
        .to_string_lossy()
        .into_owned()
}

/// Decodes the `\xNN` escapes udev puts in `/dev/disk/by-label` names. The
/// decoded bytes are read as UTF-8 (invalid bytes become U+FFFD). An escape
/// that is not a backslash, `x` and two hex digits is kept as written.
pub fn decode_udev_name(name: &OsStr) -> String {
    let b = name.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = (b[i] == b'\\' && b.get(i + 1) == Some(&b'x'))
            .then(|| b.get(i + 2..i + 4))
            .flatten()
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match hex {
            Some(v) => {
                out.push(v);
                i += 4;
            }
            None => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Filesystem labels by canonical device path, from `/dev/disk/by-label`.
/// The directory does not exist on a machine with no labelled filesystem,
/// which is no error. A link whose device is gone is a stale udev entry and
/// labels nothing.
fn labels() -> Result<HashMap<PathBuf, String>, VolumesError> {
    let dir = Path::new(BY_LABEL);
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(e) => return Err(VolumesError::io(dir, e)),
    };
    let mut map = HashMap::new();
    for entry in entries {
        let entry = entry.map_err(|e| VolumesError::io(dir, e))?;
        if let Ok(device) = std::fs::canonicalize(entry.path()) {
            map.insert(device, decode_udev_name(&entry.file_name()));
        }
    }
    Ok(map)
}

/// The passwd name of `uid`: the first field of the `/etc/passwd` line whose
/// third field is `uid`.
fn passwd_name(uid: u32) -> Result<Option<String>, VolumesError> {
    let raw = match std::fs::read(PASSWD) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(VolumesError::io(Path::new(PASSWD), e)),
    };
    Ok(raw.split(|&b| b == b'\n').find_map(|line| {
        let mut f = line.split(|&b| b == b':');
        let name = f.next()?;
        let id = f.nth(1)?;
        (std::str::from_utf8(id).ok()?.parse() == Ok(uid)).then(|| text(name))
    }))
}

/// The login name used in `/run/media/<user>`. UDisks takes it from the
/// passwd entry of the uid, so that wins; `$USER` is the fallback for a
/// system with no passwd entry (a container). A passwd file that cannot be
/// read is recorded in `problems` and `$USER` is tried.
pub(crate) fn current_user(uid: u32, problems: &mut Vec<ListProblem>) -> Option<String> {
    match passwd_name(uid) {
        Ok(Some(n)) => return Some(n),
        Ok(None) => {}
        Err(e) => problems.push(ListProblem::Passwd(e.to_string())),
    }
    let user = std::env::var("USER").ok().filter(|u| !u.is_empty());
    if user.is_none() {
        problems.push(ListProblem::NoUserName { uid });
    }
    user
}

/// Reads `/proc/self/mountinfo` as raw bytes.
pub(crate) fn read_mountinfo() -> Result<Vec<u8>, VolumesError> {
    std::fs::read(MOUNTINFO).map_err(|e| VolumesError::io(Path::new(MOUNTINFO), e))
}

/// Something `list` could not do, with the volumes it could find still
/// returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListProblem {
    /// A mountinfo line that does not follow proc(5) was skipped, so the
    /// volume it describes (if any) is missing.
    MalformedLine { line: usize, reason: &'static str },
    /// The label directory could not be read; labels fall back to mount
    /// point names.
    Labels(String),
    /// `/etc/passwd` could not be read.
    Passwd(String),
    /// No user name from passwd or `$USER`: volumes under
    /// `/run/media/<user>` cannot be told and are missing.
    NoUserName { uid: u32 },
}

/// The volumes found and what went wrong finding them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    pub mounts: Vec<Mount>,
    pub problems: Vec<ListProblem>,
}

/// Builds the listing from mountinfo bytes. `labels` is only called when a
/// mount sits on a `/dev/` device.
fn assemble(
    contents: &[u8],
    uid: u32,
    user: Option<&str>,
    is_writable: &dyn Fn(&Path) -> bool,
    labels: &dyn Fn() -> Result<HashMap<PathBuf, String>, VolumesError>,
    mut problems: Vec<ListProblem>,
) -> Listing {
    let (lines, malformed) = parse_mountinfo_report(contents);
    problems.extend(malformed);
    let mut mounts = user_mounts(&lines, uid, user.unwrap_or(""), is_writable);
    if mounts.iter().any(|m| m.device.starts_with("/dev/")) {
        match labels() {
            Ok(labels) => {
                for m in mounts.iter_mut().filter(|m| m.device.starts_with("/dev/")) {
                    let device = std::fs::canonicalize(&m.device)
                        .unwrap_or_else(|_| PathBuf::from(&m.device));
                    if let Some(label) = labels.get(&device) {
                        m.label = label.clone();
                    }
                }
            }
            Err(e) => problems.push(ListProblem::Labels(e.to_string())),
        }
    }
    Listing { mounts, problems }
}

/// The mounts of this process's namespace that a person would call volumes
/// (see [`user_mounts`] for the rule), labelled from udev's by-label links
/// when there is one for the device, else by the mount point's name.
///
/// Only failing to read `/proc/self/mountinfo` is an `Err`. A malformed
/// line, an unreadable label directory or passwd file, or an unknown user
/// name leaves the other volumes listed and is returned in
/// [`Listing::problems`], so one fault does not blank the panel.
///
/// This does blocking I/O (file reads, a `stat` and an `access(2)` probe per
/// candidate mount) and belongs off the UI thread. "Writable" is `access(2)`
/// with `W_OK`; remote filesystems outside the named trees are never probed.
pub fn list() -> Result<Listing, VolumesError> {
    let raw = read_mountinfo()?;
    let uid = rustix::process::getuid().as_raw();
    let mut problems = Vec::new();
    let user = current_user(uid, &mut problems);
    let writable = |p: &Path| rustix::fs::access(p, rustix::fs::Access::WRITE_OK).is_ok();
    Ok(assemble(
        &raw,
        uid,
        user.as_deref(),
        &writable,
        &labels,
        problems,
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    const TEXT: &[u8] = b"bad line\n\
        20 1 0:19 / / rw - btrfs /dev/nvme0n1p2 rw\n\
        25 20 8:17 / /run/media/me/STICK rw - exfat /dev/sdb1 rw\n";

    fn run(
        user: Option<&str>,
        labels: &dyn Fn() -> Result<HashMap<PathBuf, String>, VolumesError>,
    ) -> Listing {
        assemble(TEXT, 1000, user, &|_| false, labels, Vec::new())
    }

    #[test]
    fn one_malformed_line_does_not_blank_the_listing() {
        let l = run(Some("me"), &|| Ok(HashMap::new()));
        assert_eq!(l.mounts.len(), 1);
        assert_eq!(
            l.problems,
            vec![ListProblem::MalformedLine {
                line: 1,
                reason: "no - separator after the fixed fields"
            }]
        );
    }

    #[test]
    fn label_failure_falls_back_and_is_recorded() {
        let fail = || {
            Err(VolumesError::io(
                Path::new(BY_LABEL),
                std::io::Error::other("denied"),
            ))
        };
        let l = run(Some("me"), &fail);
        assert_eq!(l.mounts[0].label, "STICK");
        assert!(l
            .problems
            .iter()
            .any(|p| matches!(p, ListProblem::Labels(_))));
        let ok = || {
            Ok(HashMap::from([(
                PathBuf::from("/dev/sdb1"),
                "My Stick".to_string(),
            )]))
        };
        assert_eq!(run(Some("me"), &ok).mounts[0].label, "My Stick");
    }

    #[test]
    fn unknown_user_still_lists_what_it_can() {
        let l = run(None, &|| Ok(HashMap::new()));
        // The stick under /run/media/<user> cannot be told without a user.
        assert!(l.mounts.is_empty());
    }
}
