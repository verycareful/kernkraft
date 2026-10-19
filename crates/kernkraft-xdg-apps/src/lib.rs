// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! What kind a file is and which applications open it, from the
//! freedesktop.org databases on the machine.
//!
//! Specifications: Shared MIME-info Database 0.21 (section 2.11 for glob
//! matching, section 2.4 for `subclasses` and `aliases`) for [`MimeDb`].
//!
//! Every public function is synchronous, takes paths and owned data, and
//! reports failure as [`AppsError`].

mod error;
mod mime;

pub use error::AppsError;
pub use mime::{MimeDb, SNIFF_LEN};
