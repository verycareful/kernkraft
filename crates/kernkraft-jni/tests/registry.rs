// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The handle registry.

use std::panic::{catch_unwind, AssertUnwindSafe};

use kernkraft_jni::error::WireError;
use kernkraft_jni::registry::Registry;

#[test]
fn handles_are_distinct_and_never_zero() {
    let registry = Registry::new();

    let first = registry.insert("a");
    let second = registry.insert("b");

    assert_eq!(first, 1);
    assert_eq!(second, 2);
    assert_eq!(registry.len(), 2);
}

#[test]
fn a_removed_handle_is_not_reused() {
    let registry = Registry::new();
    let first = registry.insert(1);

    assert!(registry.remove(first));
    let second = registry.insert(2);

    assert_ne!(first, second);
    assert!(!registry.remove(first));
    assert_eq!(
        registry.with(first, |_| Ok(())).unwrap_err().kind,
        "UnknownHandle"
    );
}

#[test]
fn with_gives_the_session_its_own_state() {
    let registry = Registry::new();
    let handle = registry.insert(vec![1]);

    registry
        .with(handle, |list| {
            list.push(2);
            Ok(())
        })
        .unwrap();

    assert_eq!(
        registry.with(handle, |list| Ok(list.clone())).unwrap(),
        vec![1, 2]
    );
}

#[test]
fn a_session_that_panicked_is_refused_afterwards() {
    let registry = Registry::new();
    let handle = registry.insert(0);

    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let _ = registry.with(handle, |_| -> Result<(), WireError> { panic!("mid-edit") });
    }));

    assert!(outcome.is_err());
    let error = registry.with(handle, |_| Ok(())).unwrap_err();
    assert_eq!(error.kind, "SessionPoisoned");
}

#[test]
fn a_poisoned_session_does_not_block_another() {
    let registry = Registry::new();
    let broken = registry.insert(0);
    let fine = registry.insert(1);
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let _ = registry.with(broken, |_| -> Result<(), WireError> { panic!("mid-edit") });
    }));

    assert_eq!(registry.with(fine, |n| Ok(*n)).unwrap(), 1);
}
