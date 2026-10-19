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
/// [`parse_mountinfo_strict`] when a skipped line must not go unnoticed.
pub fn parse_mountinfo(contents: &str) -> Vec<MountInfoLine> {
    parse_each(contents.as_bytes())
        .filter_map(|(_, r)| r.ok())
        .collect()
}

/// Parses mountinfo text (or the raw bytes of the file, which may hold
/// non-UTF-8 names) and fails on the first malformed line, reporting its
/// number.
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

/// The mounts of `lines` that a person would call volumes.
///
/// Kept: everything under `/run/media/<user>`, `/media` and `/mnt`, and any
/// other mount `is_writable` accepts. Dropped, whatever `is_writable` says:
/// pseudo filesystems, `/`, `/boot*`, `/efi`, `/home` itself, anything under
/// `/proc`, `/sys` or `/dev`, and anything under `/run` other than the
/// user's media directory. A FUSE mount whose `user_id=` option names
/// another uid than `uid` belongs to someone else and is dropped.
///
/// `label` is the mount point's last component; [`list`] replaces it with
/// the filesystem label when udev has one.
pub fn user_mounts(
    lines: &[MountInfoLine],
    uid: u32,
    user: &str,
    is_writable: &dyn Fn(&Path) -> bool,
) -> Vec<Mount> {
    let media = media_dir(user);
    let under_media = |p: &Path| media.as_deref().is_some_and(|m| p.starts_with(m));
    lines
        .iter()
        .filter(|l| {
            let p = l.mount_point.as_path();
            if PSEUDO_FS.contains(&l.fs_type.as_str()) {
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
            if p == Path::new("/")
                || p == Path::new("/home")
                || p.as_os_str().as_bytes().starts_with(b"/boot")
                || p.starts_with("/efi")
                || p.starts_with("/proc")
                || p.starts_with("/sys")
                || p.starts_with("/dev")
            {
                return false;
            }
            if p.starts_with("/run") {
                return under_media(p);
            }
            p.starts_with("/media") || p.starts_with("/mnt") || is_writable(p)
        })
        .map(|l| Mount {
            mount_point: l.mount_point.clone(),
            device: l.source.clone(),
            fs_type: l.fs_type.clone(),
            label: base_name(&l.mount_point),
            removable: under_media(&l.mount_point) || l.mount_point.starts_with("/media"),
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
/// system with no passwd entry (a container).
fn current_user(uid: u32) -> Result<String, VolumesError> {
    if let Some(n) = passwd_name(uid)? {
        return Ok(n);
    }
    std::env::var("USER")
        .ok()
        .filter(|u| !u.is_empty())
        .ok_or(VolumesError::NoUser { uid })
}

/// The mounts of this process's namespace that a person would call volumes
/// (see [`user_mounts`] for the rule), labelled from udev's by-label links
/// when there is one for the device, else by the mount point's name.
///
/// A malformed mountinfo line fails the whole call
/// ([`VolumesError::MalformedMountinfo`]): the kernel writes the file, so a
/// line this crate cannot read means the volume it describes would be
/// silently missing otherwise.
///
/// "Writable" is `access(2)` with `W_OK` on the mount point; on a network
/// filesystem that goes through the mount and can block while the server is
/// unreachable.
pub fn list() -> Result<Vec<Mount>, VolumesError> {
    let raw = std::fs::read(MOUNTINFO).map_err(|e| VolumesError::io(Path::new(MOUNTINFO), e))?;
    let lines = parse_mountinfo_strict(&raw)?;
    let uid = rustix::process::getuid().as_raw();
    let user = current_user(uid)?;
    let writable = |p: &Path| rustix::fs::access(p, rustix::fs::Access::WRITE_OK).is_ok();
    let mut mounts = user_mounts(&lines, uid, &user, &writable);
    if mounts.iter().any(|m| m.device.starts_with("/dev/")) {
        let labels = labels()?;
        for m in &mut mounts {
            if !m.device.starts_with("/dev/") {
                continue;
            }
            let device =
                std::fs::canonicalize(&m.device).unwrap_or_else(|_| PathBuf::from(&m.device));
            if let Some(label) = labels.get(&device) {
                m.label = label.clone();
            }
        }
    }
    Ok(mounts)
}
