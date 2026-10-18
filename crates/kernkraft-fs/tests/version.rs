// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

const VERSION_FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../VERSION");

/// Four parts for a release, five on a dev branch (the commit index).
const RELEASE_PARTS: usize = 4;
const DEV_PARTS: usize = 5;
const SEMVER_PARTS: usize = 3;

#[test]
fn label_matches_version_file() {
    let file = std::fs::read_to_string(VERSION_FILE).unwrap();
    assert_eq!(kernkraft_fs::VERSION_LABEL, file);
    assert!(!file.ends_with('\n'));
}

#[test]
fn label_is_four_or_five_numeric_parts() {
    let parts: Vec<&str> = kernkraft_fs::VERSION_LABEL.split('.').collect();
    assert!(
        parts.len() == RELEASE_PARTS || parts.len() == DEV_PARTS,
        "label {:?} has {} parts",
        kernkraft_fs::VERSION_LABEL,
        parts.len()
    );
    for part in parts {
        assert!(!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
    }
}

#[test]
fn cargo_version_is_label_as_semver_with_build_metadata() {
    let parts: Vec<&str> = kernkraft_fs::VERSION_LABEL.split('.').collect();
    let expected = format!(
        "{}+{}",
        parts[..SEMVER_PARTS].join("."),
        parts[SEMVER_PARTS..].join(".")
    );
    assert_eq!(env!("CARGO_PKG_VERSION"), expected);
}
