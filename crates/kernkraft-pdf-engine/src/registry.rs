// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Open sessions, by the `i64` handle the application holds.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use crate::error::WireError;

/// The handle of the first session. 0 is never a handle, so a field left
/// at its default does not name a session.
const FIRST_HANDLE: i64 = 1;

/// Sessions of type `T`, each behind its own lock: a long operation on one
/// document does not hold up another.
pub struct Registry<T> {
    next: AtomicI64,
    entries: Mutex<HashMap<i64, Arc<Mutex<T>>>>,
}

impl<T> Default for Registry<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Registry<T> {
    pub fn new() -> Self {
        Self {
            next: AtomicI64::new(FIRST_HANDLE),
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Holds `session` and gives its handle. A handle is never used twice,
    /// so a stale one from a closed session cannot name a new one.
    pub fn insert(&self, session: T) -> i64 {
        let handle = self.next.fetch_add(1, Ordering::Relaxed);
        self.map().insert(handle, Arc::new(Mutex::new(session)));
        handle
    }

    /// Runs `body` on the session `handle` names. A session whose earlier
    /// operation panicked is `SessionPoisoned`: its state is in doubt.
    pub fn with<R>(
        &self,
        handle: i64,
        body: impl FnOnce(&mut T) -> Result<R, WireError>,
    ) -> Result<R, WireError> {
        let entry = self
            .map()
            .get(&handle)
            .cloned()
            .ok_or_else(|| WireError::unknown_handle(handle))?;
        let mut session = entry
            .lock()
            .map_err(|_| WireError::session_poisoned(handle))?;
        body(&mut session)
    }

    /// Drops the session `handle` names. `false` when there was none.
    pub fn remove(&self, handle: i64) -> bool {
        self.map().remove(&handle).is_some()
    }

    pub fn len(&self) -> usize {
        self.map().len()
    }

    pub fn is_empty(&self) -> bool {
        self.map().is_empty()
    }

    /// The table. A panic while it is locked leaves it whole: each step
    /// that holds the lock is one `HashMap` call.
    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<i64, Arc<Mutex<T>>>> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
