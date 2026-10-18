// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! File-system layer shared by every kernkraft consumer: atomic save,
//! FreeDesktop trash, XDG directories, bulk file operations and scanning.
//!
//! Every public function is synchronous, takes paths and owned data, and
//! reports failure as [`FsError`]. Long operations take a [`Progress`].

mod atomic;
mod error;
mod naming;
mod ops;
mod progress;
mod sys;
mod trash;
mod version;
mod xdg;

pub use atomic::{write_atomic, write_bytes_atomic, FileStamp};
pub use error::FsError;
pub use ops::{
    create_dir, create_file, delete_permanently, rename, unique_name, validate_name, OpReport,
};
pub use progress::{NoProgress, Progress};
pub use trash::{
    choose_target, decode_trash_path, encode_trash_path, parse_trashinfo, Trash, TrashListing,
    TrashTarget, TrashedItem,
};
pub use version::VERSION_LABEL;
pub use xdg::{base_dirs, base_dirs_from, parse_user_dirs, user_dirs, BaseDirs, UserDirs};
