// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! File-system layer shared by every kernkraft consumer: atomic save,
//! FreeDesktop trash, XDG directories, bulk file operations and scanning.
//!
//! Every public function is synchronous, takes paths and owned data, and
//! reports failure as [`FsError`]. Long operations take a [`Progress`].

mod error;
mod progress;
mod version;
mod xdg;

pub use error::FsError;
pub use progress::{NoProgress, Progress};
pub use version::VERSION_LABEL;
pub use xdg::{base_dirs, base_dirs_from, parse_user_dirs, user_dirs, BaseDirs, UserDirs};
