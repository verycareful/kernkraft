// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Bulk copy and move with conflict resolution, progress and cancel.
//!
//! Safety properties, in order of importance:
//!
//! - A destination file is never seen half-written. Each file is copied
//!   into a temp file in the destination directory, `fsync`ed, and renamed
//!   into place; a cancel or error drops the temp file.
//! - Nothing is replaced unless the conflict callback said `Replace` for
//!   that entry, and `Replace` only acts like for like: a file or symlink
//!   over a file or symlink (one atomic rename), a directory merged into a
//!   directory. A directory over a file, or a file over a directory, is
//!   refused with [`FsError::Exists`] instead of deleting anything.
//! - A move across devices deletes a source only after its copy has
//!   completed and been synced, with no entry inside it failed or skipped.
//! - A source is never placed inside itself: that is checked for every
//!   source before the first byte is written.

use std::fs::{self, File, FileTimes, Metadata, Permissions};
use std::io::{self, ErrorKind, Read, Write};
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use rustix::io::Errno;

use super::names::unique_name_for;
use super::OpReport;
use crate::sys::{parent_dir, rename_noreplace};
use crate::{FsError, Progress};

/// Read and write unit for file contents, and the granularity of progress
/// reports and cancel polls.
const CHUNK: usize = 1 << 20;

/// Mode of a temp file while it is filled; the source's bits are applied
/// before it is renamed into place.
const TEMP_MODE: u32 = 0o600;

/// Mode of a directory while its contents are copied in, so it stays
/// writable until the source's bits are applied at the end.
const BUILD_DIR_MODE: u32 = 0o700;

/// Permission bits `chmod(2)` sets.
const PERMISSION_BITS: u32 = 0o7777;

/// How to settle a name clash at the destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conflict {
    /// Leave the destination entry alone and skip this source.
    Skip,
    /// Replace a file or symlink; merge a directory into a directory.
    Replace,
    /// Place the source under the next free `name (n)`.
    KeepBoth,
}

/// Copies each source into `dest_dir`.
///
/// `resolve(source, existing)` is called once for each source entry whose
/// name is taken at the destination (for a merged directory, once per
/// clashing entry inside it). A source that would land on itself (copied
/// into its own directory) is copied as `name (n)` without asking.
/// `progress` receives bytes copied out of the total size of all sources,
/// with the current file as stage, and is polled for cancellation before
/// every chunk.
///
/// Returns `Err` only when nothing was started: `dest_dir` missing or not a
/// directory, or a source directory containing `dest_dir`
/// ([`FsError::IntoItself`]). Every per-entry outcome is in the report.
pub fn copy(
    sources: &[PathBuf],
    dest_dir: &Path,
    resolve: &dyn Fn(&Path, &Path) -> Conflict,
    progress: &dyn Progress,
) -> Result<OpReport, FsError> {
    Transfer::run(Mode::Copy, sources, dest_dir, resolve, progress)
}

/// Moves each source into `dest_dir`: a rename where source and destination
/// share a filesystem, else a copy followed by deleting the source.
/// Conflicts, progress, cancellation and the `Err` cases are as for
/// [`copy`]; a source already in `dest_dir` is skipped.
pub fn move_items(
    sources: &[PathBuf],
    dest_dir: &Path,
    resolve: &dyn Fn(&Path, &Path) -> Conflict,
    progress: &dyn Progress,
) -> Result<OpReport, FsError> {
    Transfer::run(Mode::Move, sources, dest_dir, resolve, progress)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Copy,
    Move,
}

/// Outcome of placing one entry.
enum Placed {
    Done(PathBuf),
    Skipped,
    /// The entry failed; the failure is already in the report.
    Failed,
    /// A directory was placed but something inside it failed or was skipped;
    /// those entries are already in the report.
    Partial,
    Cancelled,
}

struct Transfer<'a> {
    resolve: &'a dyn Fn(&Path, &Path) -> Conflict,
    progress: &'a dyn Progress,
    done_bytes: u64,
    total_bytes: u64,
    report: OpReport,
}

impl<'a> Transfer<'a> {
    fn run(
        mode: Mode,
        sources: &[PathBuf],
        dest_dir: &Path,
        resolve: &'a dyn Fn(&Path, &Path) -> Conflict,
        progress: &'a dyn Progress,
    ) -> Result<OpReport, FsError> {
        preflight(sources, dest_dir)?;
        let mut t = Transfer {
            resolve,
            progress,
            done_bytes: 0,
            total_bytes: sources.iter().map(|s| tree_size(s)).sum(),
            report: OpReport::default(),
        };
        for src in sources {
            match t.place(mode, src, dest_dir, true) {
                Placed::Done(target) => t.report.done.push((src.clone(), Some(target))),
                Placed::Skipped => t.report.skipped.push(src.clone()),
                Placed::Failed | Placed::Partial => {}
                Placed::Cancelled => {
                    t.report.cancelled = true;
                    break;
                }
            }
        }
        if !t.report.cancelled {
            t.progress.report(t.total_bytes, t.total_bytes, "");
        }
        Ok(t.report)
    }

    fn fail(&mut self, path: &Path, error: FsError) -> Placed {
        self.report.failed.push((path.to_path_buf(), error));
        Placed::Failed
    }

    /// Places `src` inside `dest_dir`, settling a clash first. `top` is
    /// true for a source the caller named (its skip is recorded by the
    /// caller); an entry inside a merged directory records its own skip.
    fn place(&mut self, mode: Mode, src: &Path, dest_dir: &Path, top: bool) -> Placed {
        let meta = match fs::symlink_metadata(src) {
            Ok(m) => m,
            Err(e) => return self.fail(src, FsError::not_found_or_io(src, e)),
        };
        let Some(name) = src.file_name() else {
            let name = src.to_string_lossy().into_owned();
            return self.fail(src, FsError::InvalidName { name });
        };
        let target = dest_dir.join(name);
        let existing = match fs::symlink_metadata(&target) {
            Ok(m) => m,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                return self.transfer(mode, src, &meta, &target, false)
            }
            Err(e) => return self.fail(&target, FsError::io(&target, e)),
        };

        let same_entry = meta.dev() == existing.dev() && meta.ino() == existing.ino();
        let choice = match (same_entry, mode) {
            (true, Mode::Copy) => Conflict::KeepBoth,
            (true, Mode::Move) => Conflict::Skip,
            (false, _) => (self.resolve)(src, &target),
        };
        let placed = match choice {
            Conflict::Skip => Placed::Skipped,
            Conflict::KeepBoth => {
                let name = name.to_string_lossy();
                let free = dest_dir.join(unique_name_for(dest_dir, &name, meta.is_dir()));
                self.transfer(mode, src, &meta, &free, false)
            }
            Conflict::Replace => match (meta.is_dir(), existing.is_dir()) {
                (true, true) => self.merge(mode, src, &target),
                (false, false) => self.transfer(mode, src, &meta, &target, true),
                _ => self.fail(src, FsError::Exists { path: target }),
            },
        };
        if matches!(placed, Placed::Skipped) && !top {
            self.report.skipped.push(src.to_path_buf());
        }
        placed
    }

    /// Puts `src` at `target`. `replace` is set only for a file or symlink
    /// replacing a file or symlink.
    fn transfer(
        &mut self,
        mode: Mode,
        src: &Path,
        meta: &Metadata,
        target: &Path,
        replace: bool,
    ) -> Placed {
        if mode == Mode::Copy {
            return self.copy_entry(src, meta, target, replace);
        }
        let renamed = if replace {
            fs::rename(src, target)
        } else {
            rename_noreplace(src, target)
        };
        match renamed {
            Ok(()) => {
                self.advance(tree_size(target), target);
                Placed::Done(target.to_path_buf())
            }
            Err(e) if e.raw_os_error() == Some(Errno::XDEV.raw_os_error()) => {
                match self.copy_entry(src, meta, target, replace) {
                    Placed::Done(t) => match delete_tree(src) {
                        Ok(()) => Placed::Done(t),
                        Err(e) => {
                            self.fail(src, e);
                            Placed::Partial
                        }
                    },
                    other => other,
                }
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => self.fail(
                src,
                FsError::Exists {
                    path: target.to_path_buf(),
                },
            ),
            Err(e) => self.fail(src, FsError::not_found_or_io(src, e)),
        }
    }

    fn copy_entry(&mut self, src: &Path, meta: &Metadata, target: &Path, replace: bool) -> Placed {
        let kind = meta.file_type();
        if kind.is_file() {
            self.copy_file(src, meta, target, replace)
        } else if kind.is_symlink() {
            match copy_symlink(src, target, replace) {
                Ok(()) => Placed::Done(target.to_path_buf()),
                Err(e) => self.fail(src, e),
            }
        } else if kind.is_dir() {
            self.copy_dir(src, meta, target)
        } else {
            let unsupported = io::Error::new(
                ErrorKind::Unsupported,
                "not a regular file, directory or symlink",
            );
            self.fail(src, FsError::io(src, unsupported))
        }
    }

    fn copy_file(&mut self, src: &Path, meta: &Metadata, target: &Path, replace: bool) -> Placed {
        let dir = parent_dir(target);
        let name = target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut input = match File::open(src) {
            Ok(f) => f,
            Err(e) => return self.fail(src, FsError::not_found_or_io(src, e)),
        };
        let temp = tempfile::Builder::new()
            .prefix(&format!(".{name}."))
            .suffix(".kk-tmp")
            .permissions(Permissions::from_mode(TEMP_MODE))
            .tempfile_in(dir);
        let mut temp = match temp {
            Ok(t) => t,
            Err(e) => return self.fail(src, FsError::io(dir, e)),
        };

        let mut buf = vec![0u8; CHUNK];
        let stage = src.to_string_lossy();
        loop {
            if self.progress.is_cancelled() {
                return Placed::Cancelled;
            }
            let n = match input.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return self.fail(src, FsError::io(src, e)),
            };
            if let Err(e) = temp.write_all(&buf[..n]) {
                return self.fail(src, FsError::io(temp.path(), e));
            }
            self.done_bytes += n as u64;
            self.progress
                .report(self.done_bytes, self.total_bytes, &stage);
        }

        let finished =
            apply_metadata(temp.as_file(), meta).and_then(|()| temp.as_file().sync_all());
        if let Err(e) = finished {
            return self.fail(src, FsError::io(temp.path(), e));
        }
        let persisted = if replace {
            temp.persist(target).map(drop)
        } else {
            temp.persist_noclobber(target).map(drop)
        };
        match persisted {
            Ok(()) => Placed::Done(target.to_path_buf()),
            Err(e) if e.error.kind() == ErrorKind::AlreadyExists => self.fail(
                src,
                FsError::Exists {
                    path: target.to_path_buf(),
                },
            ),
            Err(e) => self.fail(src, FsError::io(target, e.error)),
        }
    }

    /// Copies a directory to a new `target`. If the copy is cancelled, the
    /// directory this call created is removed again: it holds nothing but
    /// what was just copied into it.
    fn copy_dir(&mut self, src: &Path, meta: &Metadata, target: &Path) -> Placed {
        let created = fs::DirBuilder::new()
            .create(target)
            .and_then(|()| fs::set_permissions(target, Permissions::from_mode(BUILD_DIR_MODE)));
        if let Err(e) = created {
            return if e.kind() == ErrorKind::AlreadyExists {
                self.fail(
                    src,
                    FsError::Exists {
                        path: target.to_path_buf(),
                    },
                )
            } else {
                self.fail(src, FsError::io(target, e))
            };
        }
        match self.copy_children(src, target) {
            Placed::Cancelled => {
                if let Err(e) = fs::remove_dir_all(target) {
                    self.report
                        .failed
                        .push((target.to_path_buf(), FsError::io(target, e)));
                }
                Placed::Cancelled
            }
            Placed::Failed => {
                // The source could not be listed; the directory made for it
                // is still empty.
                if let Err(e) = fs::remove_dir(target) {
                    self.report
                        .failed
                        .push((target.to_path_buf(), FsError::io(target, e)));
                }
                Placed::Failed
            }
            outcome => {
                let finished = File::open(target).and_then(|d| apply_metadata(&d, meta));
                if let Err(e) = finished {
                    self.fail(target, FsError::io(target, e));
                    return Placed::Partial;
                }
                outcome
            }
        }
    }

    /// Copies every entry of `src` into the existing directory `target`.
    fn copy_children(&mut self, src: &Path, target: &Path) -> Placed {
        self.each_child(Mode::Copy, src, target)
    }

    /// Merges directory `src` into the existing directory `target`, clashes
    /// settled entry by entry. After a move whose every entry went over,
    /// the emptied source directory is removed.
    fn merge(&mut self, mode: Mode, src: &Path, target: &Path) -> Placed {
        let outcome = self.each_child(mode, src, target);
        if mode == Mode::Move && matches!(outcome, Placed::Done(_)) {
            if let Err(e) = fs::remove_dir(src) {
                self.fail(src, FsError::io(src, e));
                return Placed::Partial;
            }
        }
        outcome
    }

    fn each_child(&mut self, mode: Mode, src: &Path, target: &Path) -> Placed {
        let entries = match read_dir_sorted(src) {
            Ok(entries) => entries,
            Err(e) => return self.fail(src, e),
        };
        let mut complete = true;
        for child in entries {
            match self.place(mode, &child, target, false) {
                Placed::Done(_) => {}
                Placed::Skipped | Placed::Failed | Placed::Partial => complete = false,
                Placed::Cancelled => return Placed::Cancelled,
            }
        }
        if complete {
            Placed::Done(target.to_path_buf())
        } else {
            Placed::Partial
        }
    }

    fn advance(&mut self, bytes: u64, stage: &Path) {
        self.done_bytes += bytes;
        self.progress
            .report(self.done_bytes, self.total_bytes, &stage.to_string_lossy());
    }
}

/// Checks that `dest_dir` is a directory and that no source directory
/// contains it, before anything is written.
fn preflight(sources: &[PathBuf], dest_dir: &Path) -> Result<(), FsError> {
    let meta = fs::metadata(dest_dir).map_err(|e| FsError::not_found_or_io(dest_dir, e))?;
    if !meta.is_dir() {
        return Err(FsError::io(
            dest_dir,
            io::Error::from(ErrorKind::NotADirectory),
        ));
    }
    let dest = dest_dir
        .canonicalize()
        .map_err(|e| FsError::io(dest_dir, e))?;
    for src in sources {
        let is_real_dir = fs::symlink_metadata(src)
            .map(|m| m.is_dir())
            .unwrap_or(false);
        if !is_real_dir {
            continue;
        }
        let real = src.canonicalize().map_err(|e| FsError::io(src, e))?;
        if dest.starts_with(&real) {
            return Err(FsError::IntoItself { src: src.clone() });
        }
    }
    Ok(())
}

/// Copies permission bits and access and modification times from `meta`.
fn apply_metadata(file: &File, meta: &Metadata) -> io::Result<()> {
    file.set_permissions(Permissions::from_mode(meta.mode() & PERMISSION_BITS))?;
    file.set_times(
        FileTimes::new()
            .set_accessed(meta.accessed()?)
            .set_modified(meta.modified()?),
    )
}

/// Recreates symlink `src` at `target`. With `replace`, the new link is
/// made under a temp name and renamed over `target`, so `target` is never
/// missing in between.
fn copy_symlink(src: &Path, target: &Path, replace: bool) -> Result<(), FsError> {
    let link = fs::read_link(src).map_err(|e| FsError::not_found_or_io(src, e))?;
    if !replace {
        return symlink(&link, target).map_err(|e| {
            if e.kind() == ErrorKind::AlreadyExists {
                FsError::Exists {
                    path: target.to_path_buf(),
                }
            } else {
                FsError::io(target, e)
            }
        });
    }
    let dir = parent_dir(target);
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let pid = std::process::id();
    for i in 0u32.. {
        let temp = dir.join(format!(".{name}.{pid}.{i}.kk-tmp"));
        match symlink(&link, &temp) {
            Ok(()) => {
                return fs::rename(&temp, target).map_err(|e| match fs::remove_file(&temp) {
                    Ok(()) => FsError::io(target, e),
                    Err(cleanup) => FsError::io(&temp, cleanup),
                })
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(FsError::io(&temp, e)),
        }
    }
    Err(FsError::Exists {
        path: target.to_path_buf(),
    })
}

/// Deletes a moved source after its copy: a directory with its contents,
/// never following a symlink.
fn delete_tree(path: &Path) -> Result<(), FsError> {
    let meta = fs::symlink_metadata(path).map_err(|e| FsError::io(path, e))?;
    let removed = if meta.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    removed.map_err(|e| FsError::io(path, e))
}

/// Entries of `dir`, sorted by name so a copy runs in a stable order.
fn read_dir_sorted(dir: &Path) -> Result<Vec<PathBuf>, FsError> {
    let mut entries = fs::read_dir(dir)
        .map_err(|e| FsError::io(dir, e))?
        .map(|e| e.map(|e| e.path()))
        .collect::<io::Result<Vec<_>>>()
        .map_err(|e| FsError::io(dir, e))?;
    entries.sort();
    Ok(entries)
}

/// Bytes in the regular files under `path`, symlinks not followed. Entries
/// that cannot be read count as zero here; copying them reports the error.
fn tree_size(path: &Path) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.is_file() {
        return meta.len();
    }
    if !meta.is_dir() {
        return 0;
    }
    fs::read_dir(path)
        .map(|entries| entries.flatten().map(|e| tree_size(&e.path())).sum())
        .unwrap_or(0)
}
