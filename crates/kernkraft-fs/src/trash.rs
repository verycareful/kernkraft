// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! FreeDesktop.org Trash, per the Trash specification 1.0
//! (https://specifications.freedesktop.org/trash-spec/1.0/).
//!
//! A trash directory holds `files/` (the trashed items, under their trash
//! name) and `info/` (one `<name>.trashinfo` per item, recording where it
//! came from and when). A file on the home trash's device goes to the home
//! trash, `$XDG_DATA_HOME/Trash`. A file on another device goes to its
//! mount's top directory: `$topdir/.Trash/$uid` when the administrator has
//! set up a shared `.Trash` (a real directory with the sticky bit), else
//! `$topdir/.Trash-$uid`. Files are only ever renamed into a trash, never
//! copied across devices.

use std::ffi::OsString;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{ErrorKind, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use percent_encoding::{percent_decode_str, percent_encode, AsciiSet, NON_ALPHANUMERIC};

use crate::naming::numbered;
use crate::sys::{rename_exclusive, rename_noreplace, uid};
use crate::undo::Change;
use crate::{BaseDirs, FsError};

const FILES_DIR: &str = "files";
const INFO_DIR: &str = "info";
const INFO_EXT: &str = ".trashinfo";
const INFO_HEADER: &str = "[Trash Info]";
const DELETION_DATE_FORMAT: &str = "%Y-%m-%dT%H:%M:%S";
const MOUNTS_FILE: &str = "/proc/self/mounts";

/// Mode of every trash directory kernkraft creates: owner only (spec,
/// "Trash directories").
const TRASH_DIR_MODE: u32 = 0o700;
/// Mode of a `.trashinfo` file: owner read and write.
const INFO_FILE_MODE: u32 = 0o600;
/// `S_ISVTX`, the sticky bit the spec requires on a shared `$topdir/.Trash`.
const STICKY_BIT: u32 = 0o1000;

/// Bytes kept as they are in a `Path=` value: RFC 3986 unreserved
/// characters and the path separator. Everything else is `%XX`.
const PATH_KEEP: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~')
    .remove(b'/');

/// Where a trashed file goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrashTarget {
    /// The home trash.
    Home,
    /// `$topdir/.Trash/$uid`, inside an administrator-provided shared trash.
    TopDir(PathBuf),
    /// `$topdir/.Trash-$uid`, the per-user trash on that mount.
    TopDirUser(PathBuf),
}

/// One item in a trash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrashedItem {
    /// Name under `files/` (and `info/<name>.trashinfo`).
    pub name: String,
    /// Absolute path the item was trashed from.
    pub original: PathBuf,
    /// The raw `DeletionDate=` value, `YYYY-MM-DDThh:mm:ss` in local time.
    pub deleted: String,
    /// The trash directory holding the item.
    pub trash_dir: PathBuf,
    pub is_dir: bool,
}

/// Everything [`Trash::list`] found: the items, and each info file it could
/// not turn into one, with the reason.
#[derive(Debug, Default)]
pub struct TrashListing {
    pub items: Vec<TrashedItem>,
    pub unreadable: Vec<(PathBuf, FsError)>,
}

/// The user's trash: the home trash plus the per-mount trashes.
#[derive(Clone, Debug)]
pub struct Trash {
    home: PathBuf,
    uid: u32,
    /// Mount points whose trash directories `list` reads. `None` reads them
    /// from `/proc/self/mounts` at each call.
    topdirs: Option<Vec<PathBuf>>,
}

impl Trash {
    /// The trash of the current user, with its home trash at
    /// `<data>/Trash`.
    pub fn new(base: &BaseDirs) -> Trash {
        Trash {
            home: base.data.join("Trash"),
            uid: uid(),
            topdirs: None,
        }
    }

    /// A trash whose home trash is `dir` and which lists no mount trashes.
    pub fn with_home_trash(dir: PathBuf) -> Trash {
        Self::with_dirs(dir, Vec::new())
    }

    /// A trash with home trash `dir` that lists the trashes of exactly the
    /// given mount points.
    pub fn with_dirs(dir: PathBuf, topdirs: Vec<PathBuf>) -> Trash {
        Trash {
            home: dir,
            uid: uid(),
            topdirs: Some(topdirs),
        }
    }

    /// Moves `path` (a file, directory or symlink; a symlink itself is
    /// trashed, never its target) into the trash for its device.
    pub fn put(&self, path: &Path) -> Result<TrashedItem, FsError> {
        let original = absolute_with_real_parent(path)?;
        let name = original
            .file_name()
            .ok_or_else(|| FsError::InvalidName {
                name: path.to_string_lossy().into_owned(),
            })?
            .to_string_lossy()
            .into_owned();
        let meta =
            fs::symlink_metadata(&original).map_err(|e| FsError::not_found_or_io(&original, e))?;

        create_private_dir_all(&self.home)?;
        let home_dev = fs::metadata(&self.home)
            .map_err(|e| FsError::io(&self.home, e))?
            .dev();
        let topdir = if meta.dev() == home_dev {
            PathBuf::new()
        } else {
            find_topdir(&original, meta.dev())?
        };
        let target = choose_target(meta.dev(), home_dev, &topdir, self.uid, shared_trash_ok);
        let (trash_dir, recorded) = match &target {
            TrashTarget::Home => (self.home.clone(), original.clone()),
            TrashTarget::TopDir(dir) | TrashTarget::TopDirUser(dir) => {
                let dir = self.prepare_topdir_trash(&target, dir, &topdir, &original)?;
                let relative = original
                    .strip_prefix(&topdir)
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|_| original.clone());
                (dir, relative)
            }
        };
        let files = trash_dir.join(FILES_DIR);
        let info = trash_dir.join(INFO_DIR);
        create_private_dir_all(&files)?;
        create_private_dir_all(&info)?;

        let deleted = chrono::Local::now()
            .format(DELETION_DATE_FORMAT)
            .to_string();
        let contents = format!(
            "{INFO_HEADER}\nPath={}\nDeletionDate={deleted}\n",
            encode_trash_path(&recorded)
        );
        let (trash_name, info_path) = reserve_info(&files, &info, &name, meta.is_dir(), &contents)?;

        if let Err(e) = rename_noreplace(&original, &files.join(&trash_name)) {
            let cleanup = fs::remove_file(&info_path);
            return Err(match cleanup {
                Ok(()) => FsError::io(&original, e),
                Err(c) => FsError::io(&info_path, c),
            });
        }
        Ok(TrashedItem {
            name: trash_name,
            original,
            deleted,
            trash_dir,
            is_dir: meta.is_dir(),
        })
    }

    /// For a top-directory target, creates (or checks) its trash directory.
    /// A shared `.Trash/$uid` that cannot be created falls through to
    /// `.Trash-$uid`, as the spec prescribes; if that is unusable too the
    /// item is refused rather than copied elsewhere.
    fn prepare_topdir_trash(
        &self,
        target: &TrashTarget,
        dir: &Path,
        topdir: &Path,
        original: &Path,
    ) -> Result<PathBuf, FsError> {
        if let TrashTarget::TopDir(_) = target {
            if create_private_dir_all(dir).is_ok() && owned_real_dir(dir, self.uid) {
                return Ok(dir.to_path_buf());
            }
        }
        let user_dir = topdir.join(format!(".Trash-{}", self.uid));
        match DirBuilder::new().mode(TRASH_DIR_MODE).create(&user_dir) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(_) => {
                return Err(FsError::TrashUnavailable {
                    path: original.to_path_buf(),
                })
            }
        }
        if owned_real_dir(&user_dir, self.uid) {
            Ok(user_dir)
        } else {
            Err(FsError::TrashUnavailable {
                path: original.to_path_buf(),
            })
        }
    }

    /// Every item in the home trash and in the trashes of each mount point.
    /// An info file that cannot be read or parsed, or whose item is missing
    /// from `files/`, is reported in [`TrashListing::unreadable`].
    pub fn list(&self) -> Result<TrashListing, FsError> {
        let mut listing = TrashListing::default();
        read_trash_dir(&self.home, None, &mut listing)?;
        let topdirs = match &self.topdirs {
            Some(dirs) => dirs.clone(),
            None => mount_points()?,
        };
        for topdir in topdirs {
            for dir in [
                topdir.join(".Trash").join(self.uid.to_string()),
                topdir.join(format!(".Trash-{}", self.uid)),
            ] {
                if dir != self.home {
                    read_trash_dir(&dir, Some(&topdir), &mut listing)?;
                }
            }
        }
        Ok(listing)
    }

    /// Moves `item` back to its original path, creating missing parent
    /// directories. Refuses with [`FsError::Exists`] if something now
    /// occupies that path; nothing is replaced.
    pub fn restore(&self, item: &TrashedItem) -> Result<PathBuf, FsError> {
        let (file, info) = item_paths(item);
        if let Some(parent) = item.original.parent() {
            fs::create_dir_all(parent).map_err(|e| FsError::io(parent, e))?;
        }
        rename_noreplace(&file, &item.original).map_err(|e| {
            if e.kind() == ErrorKind::AlreadyExists {
                FsError::Exists {
                    path: item.original.clone(),
                }
            } else {
                FsError::not_found_or_io(&file, e)
            }
        })?;
        fs::remove_file(&info).map_err(|e| FsError::io(&info, e))?;
        Ok(item.original.clone())
    }

    /// Deletes `item` from the trash for good. A trashed directory is
    /// removed without following any symlink inside it.
    pub fn purge(&self, item: &TrashedItem) -> Result<(), FsError> {
        let (file, info) = item_paths(item);
        let meta = fs::symlink_metadata(&file).map_err(|e| FsError::not_found_or_io(&file, e))?;
        let removed = if meta.is_dir() {
            fs::remove_dir_all(&file)
        } else {
            fs::remove_file(&file)
        };
        removed.map_err(|e| FsError::io(&file, e))?;
        fs::remove_file(&info).map_err(|e| FsError::io(&info, e))
    }
}

/// [`Trash::restore`] for undo: the item is renamed back with
/// [`rename_exclusive`], so a filesystem that cannot refuse a replace in the
/// same step as the rename is refused rather than raced. Once the item is
/// back, a failure to remove its info file does not undo the restore; it is
/// returned as `Ok(Some(warning))`.
pub(crate) fn restore_exclusive(item: &TrashedItem) -> Result<Option<FsError>, FsError> {
    let (file, info) = item_paths(item);
    if let Some(parent) = item.original.parent() {
        fs::create_dir_all(parent).map_err(|e| FsError::io(parent, e))?;
    }
    rename_exclusive(&file, &item.original).map_err(|e| {
        if e.kind() == ErrorKind::AlreadyExists {
            FsError::Exists {
                path: item.original.clone(),
            }
        } else {
            FsError::not_found_or_io(&file, e)
        }
    })?;
    Ok(fs::remove_file(&info).err().map(|e| FsError::io(&info, e)))
}

/// Checks that `item` is still in its trash as the same entry: the file
/// under `files/` exists, and the info file names the same original path
/// and deletion date. A trash name is reused once its entry is emptied, so
/// the name alone does not identify the item. Returns the first difference,
/// naming the trash file or info file.
pub(crate) fn verify_entry(item: &TrashedItem) -> Result<Option<(PathBuf, Change)>, FsError> {
    let (file, info) = item_paths(item);
    match fs::symlink_metadata(&file) {
        Ok(_) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Some((file, Change::Missing))),
        Err(e) => return Err(FsError::io(&file, e)),
    }
    let contents = match fs::read_to_string(&info) {
        Ok(c) => c,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Some((info, Change::Missing))),
        Err(e) if e.kind() == ErrorKind::InvalidData => return Ok(Some((info, Change::Modified))),
        Err(e) => return Err(FsError::io(&info, e)),
    };
    let Some((raw_path, deleted)) = parse_trashinfo(&contents) else {
        return Ok(Some((info, Change::Modified)));
    };
    let recorded = decode_trash_path(&raw_path);
    // A mount trash records the path relative to the mount's top directory.
    let same_path = if recorded.is_absolute() {
        recorded == item.original
    } else {
        !recorded.as_os_str().is_empty() && item.original.ends_with(&recorded)
    };
    if same_path && deleted == item.deleted {
        Ok(None)
    } else {
        Ok(Some((info, Change::Modified)))
    }
}

/// The [`TrashedItem`] for an item trashed from `original` and now held at
/// `held`, which is `<trash dir>/files/<name>`. The deletion date is read
/// from the item's info file and left empty if that cannot be read; the
/// item then fails [`verify_entry`], so an entry that cannot be identified
/// is never restored.
pub(crate) fn item_at(original: &Path, held: &Path) -> TrashedItem {
    let name = held
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let trash_dir = held
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let info = trash_dir.join(INFO_DIR).join(format!("{name}{INFO_EXT}"));
    let deleted = fs::read_to_string(info)
        .ok()
        .and_then(|c| parse_trashinfo(&c))
        .map(|(_, date)| date)
        .unwrap_or_default();
    TrashedItem {
        name,
        original: original.to_path_buf(),
        deleted,
        trash_dir,
        is_dir: fs::symlink_metadata(held).is_ok_and(|m| m.is_dir()),
    }
}

/// Picks the trash for a file on device `file_dev`, given the home trash's
/// device and the file's mount top directory. `shared_trash_ok` tells
/// whether `$topdir/.Trash` passes the spec's checks.
pub fn choose_target(
    file_dev: u64,
    home_trash_dev: u64,
    topdir: &Path,
    uid: u32,
    shared_trash_ok: impl Fn(&Path) -> bool,
) -> TrashTarget {
    if file_dev == home_trash_dev {
        return TrashTarget::Home;
    }
    let shared = topdir.join(".Trash");
    if shared_trash_ok(&shared) {
        TrashTarget::TopDir(shared.join(uid.to_string()))
    } else {
        TrashTarget::TopDirUser(topdir.join(format!(".Trash-{uid}")))
    }
}

/// The `Path=` encoding: every byte except RFC 3986 unreserved characters
/// and `/` becomes `%XX`, so any file name (UTF-8 or not) survives.
pub fn encode_trash_path(p: &Path) -> String {
    percent_encode(p.as_os_str().as_bytes(), PATH_KEEP).to_string()
}

/// Reverses [`encode_trash_path`] (and accepts any other valid
/// percent-encoding of the same bytes).
pub fn decode_trash_path(s: &str) -> PathBuf {
    PathBuf::from(OsString::from_vec(percent_decode_str(s).collect()))
}

/// Reads the `Path=` and `DeletionDate=` values of the `[Trash Info]` group,
/// which must be the file's first group. `None` when either is missing.
/// The file is in the Desktop Entry format (Trash spec, "Contents of a
/// trash directory"), which ignores space before and after the `=`; the
/// first of a repeated key counts.
pub fn parse_trashinfo(contents: &str) -> Option<(String, String)> {
    let mut lines = contents.lines().map(str::trim).filter(|l| !l.is_empty());
    if lines.next()? != INFO_HEADER {
        return None;
    }
    let (mut path, mut date) = (None, None);
    for line in lines.take_while(|l| !l.starts_with('[')) {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let slot = match key.trim_end() {
            "Path" => &mut path,
            "DeletionDate" => &mut date,
            _ => continue,
        };
        slot.get_or_insert_with(|| value.trim_start().to_string());
    }
    Some((path?, date?))
}

/// Creates `info/<name>.trashinfo` exclusively (`O_EXCL`), counting up the
/// name until neither the info file nor `files/<name>` is taken, and writes
/// `contents` to it. Creating the info file first is what reserves the name
/// against other programs trashing at the same moment (spec, "Contents of a
/// trash directory").
fn reserve_info(
    files: &Path,
    info: &Path,
    name: &str,
    is_dir: bool,
    contents: &str,
) -> Result<(String, PathBuf), FsError> {
    let mut n = 1;
    loop {
        let candidate = if n == 1 {
            name.to_string()
        } else {
            numbered(name, n, !is_dir)
        };
        let info_path = info.join(format!("{candidate}{INFO_EXT}"));
        let opened = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(INFO_FILE_MODE)
            .open(&info_path);
        match opened {
            Ok(mut file) => {
                if fs::symlink_metadata(files.join(&candidate)).is_ok() {
                    fs::remove_file(&info_path).map_err(|e| FsError::io(&info_path, e))?;
                } else {
                    let written = file
                        .write_all(contents.as_bytes())
                        .and_then(|()| file.sync_all());
                    if let Err(e) = written {
                        return Err(match fs::remove_file(&info_path) {
                            Ok(()) => FsError::io(&info_path, e),
                            Err(cleanup) => FsError::io(&info_path, cleanup),
                        });
                    }
                    return Ok((candidate, info_path));
                }
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(FsError::io(&info_path, e)),
        }
        n += 1;
    }
}

fn read_trash_dir(
    dir: &Path,
    topdir: Option<&Path>,
    listing: &mut TrashListing,
) -> Result<(), FsError> {
    let info_dir = dir.join(INFO_DIR);
    let entries = match fs::read_dir(&info_dir) {
        Ok(entries) => entries,
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::PermissionDenied) => {
            return Ok(())
        }
        Err(e) => return Err(FsError::io(&info_dir, e)),
    };
    for entry in entries {
        let entry = entry.map_err(|e| FsError::io(&info_dir, e))?;
        let file_name = entry.file_name();
        if !file_name.as_bytes().ends_with(INFO_EXT.as_bytes()) {
            continue;
        }
        // TrashedItem names are text; a trash name that is not UTF-8 (put by
        // another program) is reported rather than passed over.
        let Some(name) = file_name
            .to_str()
            .and_then(|n| n.strip_suffix(INFO_EXT))
            .map(str::to_string)
        else {
            let name = file_name.to_string_lossy().into_owned();
            listing
                .unreadable
                .push((entry.path(), FsError::InvalidName { name }));
            continue;
        };
        match read_item(dir, topdir, &entry.path(), name) {
            Ok(item) => listing.items.push(item),
            Err(e) => listing.unreadable.push((entry.path(), e)),
        }
    }
    Ok(())
}

fn read_item(
    dir: &Path,
    topdir: Option<&Path>,
    info_path: &Path,
    name: String,
) -> Result<TrashedItem, FsError> {
    let contents = fs::read_to_string(info_path).map_err(|e| FsError::io(info_path, e))?;
    let bad = || FsError::BadTrashInfo {
        path: info_path.to_path_buf(),
    };
    let (raw_path, deleted) = parse_trashinfo(&contents).ok_or_else(bad)?;
    let recorded = decode_trash_path(&raw_path);
    let original = match topdir {
        _ if recorded.is_absolute() => recorded,
        Some(top) => top.join(recorded),
        None => return Err(bad()),
    };
    let file = dir.join(FILES_DIR).join(&name);
    let meta = fs::symlink_metadata(&file).map_err(|e| FsError::not_found_or_io(&file, e))?;
    Ok(TrashedItem {
        name,
        original,
        deleted,
        trash_dir: dir.to_path_buf(),
        is_dir: meta.is_dir(),
    })
}

fn item_paths(item: &TrashedItem) -> (PathBuf, PathBuf) {
    (
        item.trash_dir.join(FILES_DIR).join(&item.name),
        item.trash_dir
            .join(INFO_DIR)
            .join(format!("{}{INFO_EXT}", item.name)),
    )
}

/// `path` made absolute with its parent directory resolved through any
/// symlinks, while the last component stays as given (so a symlink is
/// trashed as itself).
fn absolute_with_real_parent(path: &Path) -> Result<PathBuf, FsError> {
    let abs = std::path::absolute(path).map_err(|e| FsError::io(path, e))?;
    let (Some(parent), Some(name)) = (abs.parent(), abs.file_name()) else {
        return Err(FsError::InvalidName {
            name: path.to_string_lossy().into_owned(),
        });
    };
    if name == ".." {
        return Err(FsError::InvalidName {
            name: path.to_string_lossy().into_owned(),
        });
    }
    let real = parent
        .canonicalize()
        .map_err(|e| FsError::not_found_or_io(parent, e))?;
    Ok(real.join(name))
}

/// The mount's top directory for `path`: the highest ancestor still on
/// device `dev`.
fn find_topdir(path: &Path, dev: u64) -> Result<PathBuf, FsError> {
    let mut top = path.parent().unwrap_or(path).to_path_buf();
    while let Some(parent) = top.parent() {
        let meta = fs::symlink_metadata(parent).map_err(|e| FsError::io(parent, e))?;
        if meta.dev() != dev {
            break;
        }
        top = parent.to_path_buf();
    }
    Ok(top)
}

/// The spec's checks on `$topdir/.Trash`: it exists, is a directory and not
/// a symlink, and has the sticky bit set.
fn shared_trash_ok(shared: &Path) -> bool {
    fs::symlink_metadata(shared)
        .map(|m| m.is_dir() && m.mode() & STICKY_BIT != 0)
        .unwrap_or(false)
}

/// A real directory (not a symlink) owned by `uid`.
fn owned_real_dir(dir: &Path, uid: u32) -> bool {
    fs::symlink_metadata(dir)
        .map(|m| m.is_dir() && m.uid() == uid)
        .unwrap_or(false)
}

fn create_private_dir_all(dir: &Path) -> Result<(), FsError> {
    DirBuilder::new()
        .recursive(true)
        .mode(TRASH_DIR_MODE)
        .create(dir)
        .map_err(|e| FsError::io(dir, e))
}

/// Mount points from `/proc/self/mounts`, with the kernel's octal escapes
/// (`\040` for a space, and so on) decoded.
fn mount_points() -> Result<Vec<PathBuf>, FsError> {
    let text = fs::read(MOUNTS_FILE).map_err(|e| FsError::io(Path::new(MOUNTS_FILE), e))?;
    Ok(text
        .split(|&b| b == b'\n')
        .filter_map(|line| line.split(|&b| b == b' ').nth(1))
        .map(|field| PathBuf::from(OsString::from_vec(unescape_mount_field(field))))
        .collect())
}

fn unescape_mount_field(field: &[u8]) -> Vec<u8> {
    const OCTAL_ESCAPE_LEN: usize = 4;
    let mut out = Vec::with_capacity(field.len());
    let mut i = 0;
    while i < field.len() {
        let esc = field.get(i..i + OCTAL_ESCAPE_LEN);
        let value = esc
            .filter(|e| e[0] == b'\\' && e[1..].iter().all(|d| (b'0'..=b'7').contains(d)))
            .map(|e| {
                e[1..]
                    .iter()
                    .fold(0u32, |acc, d| acc * 8 + u32::from(d - b'0'))
            })
            .and_then(|v| u8::try_from(v).ok());
        match value {
            Some(byte) => {
                out.push(byte);
                i += OCTAL_ESCAPE_LEN;
            }
            None => {
                out.push(field[i]);
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::unescape_mount_field;

    #[test]
    fn mount_field_escapes_decode() {
        assert_eq!(unescape_mount_field(br"/mnt/my\040disk"), b"/mnt/my disk");
        assert_eq!(unescape_mount_field(br"/a\134b"), br"/a\b");
        assert_eq!(unescape_mount_field(br"/odd\04"), br"/odd\04");
    }
}
