// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! A private staging directory for temporary copies of user documents
//! (a renderer's working copy, a file received from another app).
//!
//! The directory is `<cache_home>/<app>/staging`, mode `0700`, owned by
//! the current user and not a symlink: anything else is refused, since a
//! staging directory another user could read or redirect would leak the
//! documents copied into it. It is emptied when opened and on request.

use std::fs::{self, DirBuilder, File, Permissions};
use std::io::{self, ErrorKind};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::ops::validate_name;
use crate::sys::uid;
use crate::FsError;

const STAGING_DIR: &str = "staging";
/// Owner-only directory.
const PRIVATE_DIR_MODE: u32 = 0o700;
/// Owner read and write.
const PRIVATE_FILE_MODE: u32 = 0o600;
const PERMISSION_BITS: u32 = 0o777;

#[derive(Debug)]
pub struct PrivateCache {
    dir: PathBuf,
}

impl PrivateCache {
    /// Opens (creating as needed) `<cache_home>/<app>/staging` and empties
    /// it, sweeping copies left by a previous run.
    ///
    /// Refuses with [`FsError::UnsafeCacheDir`] when `<app>` or the staging
    /// directory is a symlink, not a directory, or owned by another user.
    /// A staging directory whose mode is not `0700` is reset to it.
    pub fn open(cache_home: &Path, app: &str) -> Result<PrivateCache, FsError> {
        validate_name(app)?;
        fs::create_dir_all(cache_home).map_err(|e| FsError::io(cache_home, e))?;
        let app_dir = cache_home.join(app);
        ensure_owned_dir(&app_dir)?;
        let dir = app_dir.join(STAGING_DIR);
        ensure_owned_dir(&dir)?;
        let mode = fs::symlink_metadata(&dir)
            .map_err(|e| FsError::io(&dir, e))?
            .mode();
        if mode & PERMISSION_BITS != PRIVATE_DIR_MODE {
            fs::set_permissions(&dir, Permissions::from_mode(PRIVATE_DIR_MODE))
                .map_err(|e| FsError::io(&dir, e))?;
        }
        let cache = PrivateCache { dir };
        cache.sweep()?;
        Ok(cache)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Copies `src` into the staging directory under a fresh name that
    /// keeps its extension, mode `0600`. Returns the copy's path.
    pub fn stage_copy(&self, src: &Path) -> Result<PathBuf, FsError> {
        let stem = src
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let suffix = src
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        let mut input = File::open(src).map_err(|e| FsError::not_found_or_io(src, e))?;
        let mut temp = tempfile::Builder::new()
            .prefix(&format!("{stem}-"))
            .suffix(&suffix)
            .permissions(Permissions::from_mode(PRIVATE_FILE_MODE))
            .tempfile_in(&self.dir)
            .map_err(|e| FsError::io(&self.dir, e))?;
        io::copy(&mut input, &mut temp).map_err(|e| FsError::io(src, e))?;
        let (_, path) = temp.keep().map_err(|e| FsError::io(&self.dir, e.error))?;
        Ok(path)
    }

    /// Removes everything in the staging directory, never following a
    /// symlink. Every entry is attempted; the first failure is returned.
    pub fn sweep(&self) -> Result<(), FsError> {
        let mut first_error = None;
        let entries = fs::read_dir(&self.dir).map_err(|e| FsError::io(&self.dir, e))?;
        for entry in entries {
            let path = match entry {
                Ok(e) => e.path(),
                Err(e) => {
                    first_error.get_or_insert(FsError::io(&self.dir, e));
                    continue;
                }
            };
            let removed = fs::symlink_metadata(&path).and_then(|m| {
                if m.is_dir() {
                    fs::remove_dir_all(&path)
                } else {
                    fs::remove_file(&path)
                }
            });
            match removed {
                Ok(()) => {}
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => {
                    first_error.get_or_insert(FsError::io(&path, e));
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

/// Creates `dir` (mode `0700`) if missing, then checks it is a real
/// directory owned by the current user.
fn ensure_owned_dir(dir: &Path) -> Result<(), FsError> {
    match DirBuilder::new().mode(PRIVATE_DIR_MODE).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
        Err(e) => return Err(FsError::io(dir, e)),
    }
    let meta = fs::symlink_metadata(dir).map_err(|e| FsError::io(dir, e))?;
    if meta.is_dir() && meta.uid() == uid() {
        Ok(())
    } else {
        Err(FsError::UnsafeCacheDir {
            path: dir.to_path_buf(),
        })
    }
}
