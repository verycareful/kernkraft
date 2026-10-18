// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

/// The full version label from the workspace `VERSION` file, for example
/// `0.1.0.0`. Cargo's own version carries the same label as semver.
pub const VERSION_LABEL: &str = include_str!("../../../VERSION");
