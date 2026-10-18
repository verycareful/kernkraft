// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

/// Progress sink and cancel poll for long operations.
///
/// `report` receives `done` out of `total` units (bytes or items, as the
/// operation documents) and a short stage label. Operations poll
/// `is_cancelled` at safe points and stop without leaving partial output.
pub trait Progress: Sync {
    fn report(&self, done: u64, total: u64, stage: &str);

    fn is_cancelled(&self) -> bool {
        false
    }
}

/// A [`Progress`] that ignores reports and never cancels.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoProgress;

impl Progress for NoProgress {
    fn report(&self, _done: u64, _total: u64, _stage: &str) {}
}
