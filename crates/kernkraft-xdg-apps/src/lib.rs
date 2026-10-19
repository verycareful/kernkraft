// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! What kind a file is and which applications open it, from the
//! freedesktop.org databases on the machine.
//!
//! Specifications: Shared MIME-info Database 0.21 (section 2.11 for glob
//! matching, section 2.4 for `subclasses` and `aliases`) for [`MimeDb`];
//! Desktop Entry Specification 1.5 for [`DesktopEntry`] and [`expand_exec`];
//! MIME Applications Associations 1.0.1 for [`AppRegistry`].
//!
//! Every public function is synchronous, takes paths and owned data, and
//! reports failure as [`AppsError`].

mod desktop_entry;
mod error;
mod exec;
mod launch;
mod mime;
mod mimeapps;
mod terminal;

pub use desktop_entry::{desktop_id, parse_desktop_entry, DesktopEntry};
pub use error::AppsError;
pub use exec::expand_exec;
pub use launch::{launch, launch_in_terminal, set_default, set_default_command};
pub use mime::{MimeDb, SNIFF_LEN};
pub use mimeapps::AppRegistry;
pub use terminal::{open_terminal, terminal_command, terminal_command_for};
