// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Merge, split, extract and compact a file through the string layer the exports wrap.
//! Expectations come from the fixture: `testdata/kern-sample.pdf` has two
//! pages.

mod common;
mod fixture;

use std::fs;

use common::{failure_kind, parse};
use fixture::{pages_of, rebuilt_sample, sample};
use kernkraft_jni::bridge::Bridge;
use kernkraft_jni::stateless::{compact_file, extract_pages, merge_files, split_by_ranges};
use serde_json::{json, Value};

const SAMPLE_PAGES: u32 = 2;

fn path_of(value: &Value) -> String {
    value.as_str().expect("a path is a string").to_owned()
}

#[test]
fn merging_a_file_with_itself_doubles_its_pages() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("merged.pdf");
    let source = json!({ "path": sample() });
    let request = json!({ "sources": [source, source], "out": out });

    let answer = parse(&merge_files(&request.to_string()));

    assert_eq!(answer["ok"], Value::Bool(true));
    assert_eq!(answer["paths"], json!([out]));
    assert_eq!(pages_of(&out), 2 * SAMPLE_PAGES);
}

#[test]
fn merge_never_replaces_a_file_that_is_there() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("merged.pdf");
    fs::write(&out, b"the user's file").unwrap();
    let source = json!({ "path": sample() });
    let request = json!({ "sources": [source, source], "out": out });

    let kind = failure_kind(&merge_files(&request.to_string()));

    assert_eq!(kind, "Exists");
    assert_eq!(fs::read(&out).unwrap(), b"the user's file");
}

#[test]
fn merge_of_one_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("merged.pdf");
    let request = json!({ "sources": [{ "path": sample() }], "out": out });

    assert_eq!(failure_kind(&merge_files(&request.to_string())), "Invalid");
    assert!(!out.exists());
}

#[test]
fn extract_gives_the_listed_pages() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("two-one.pdf");
    let request = json!({ "path": sample(), "pages": [2, 1], "out": out });

    let answer = parse(&extract_pages(&request.to_string()));

    assert_eq!(answer["ok"], Value::Bool(true));
    assert_eq!(pages_of(&out), 2);
}

#[test]
fn extract_refuses_a_page_listed_twice_and_makes_no_file() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("twice.pdf");
    let request = json!({ "path": sample(), "pages": [1, 1], "out": out });

    assert_eq!(
        failure_kind(&extract_pages(&request.to_string())),
        "DuplicatePage"
    );
    assert!(!out.exists());
}

#[test]
fn extract_of_a_page_past_the_end_makes_no_file() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("none.pdf");
    let request = json!({ "path": sample(), "pages": [SAMPLE_PAGES + 1], "out": out });

    assert_eq!(
        failure_kind(&extract_pages(&request.to_string())),
        "PageOutOfRange"
    );
    assert!(!out.exists());
}

#[test]
fn a_missing_source_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let request = json!({
        "path": dir.path().join("absent.pdf"),
        "pages": [1],
        "out": dir.path().join("out.pdf"),
    });

    assert_eq!(
        failure_kind(&extract_pages(&request.to_string())),
        "NotFound"
    );
}

#[test]
fn split_names_one_file_per_range() {
    let dir = tempfile::tempdir().unwrap();
    let request = json!({ "path": sample(), "ranges": "1,2", "out_dir": dir.path() });

    let answer = parse(&split_by_ranges(&request.to_string()));

    let paths: Vec<String> = answer["paths"]
        .as_array()
        .unwrap()
        .iter()
        .map(path_of)
        .collect();
    assert_eq!(
        paths,
        [
            dir.path().join("kern-sample_1.pdf").to_string_lossy(),
            dir.path().join("kern-sample_2.pdf").to_string_lossy()
        ]
    );
    for path in &paths {
        assert_eq!(pages_of(path.as_ref()), 1);
    }
}

#[test]
fn split_range_names_the_first_and_last_page() {
    let dir = tempfile::tempdir().unwrap();
    let request = json!({ "path": sample(), "ranges": "1-2", "out_dir": dir.path() });

    let answer = parse(&split_by_ranges(&request.to_string()));

    let out = dir.path().join("kern-sample_1-2.pdf");
    assert_eq!(answer["paths"], json!([out]));
    assert_eq!(pages_of(&out), SAMPLE_PAGES);
}

#[test]
fn split_refuses_two_ranges_that_name_one_file() {
    let dir = tempfile::tempdir().unwrap();
    let request = json!({ "path": sample(), "ranges": "1,1", "out_dir": dir.path() });

    assert_eq!(
        failure_kind(&split_by_ranges(&request.to_string())),
        "BadRequest"
    );
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn split_that_fails_part_way_removes_what_it_made_and_keeps_what_was_there() {
    let dir = tempfile::tempdir().unwrap();
    let taken = dir.path().join("kern-sample_2.pdf");
    fs::write(&taken, b"already here").unwrap();
    let request = json!({ "path": sample(), "ranges": "1,2", "out_dir": dir.path() });

    let kind = failure_kind(&split_by_ranges(&request.to_string()));

    assert_eq!(kind, "Exists");
    assert!(!dir.path().join("kern-sample_1.pdf").exists());
    assert_eq!(fs::read(&taken).unwrap(), b"already here");
}

#[test]
fn split_range_past_the_end_makes_no_file() {
    let dir = tempfile::tempdir().unwrap();
    let request = json!({ "path": sample(), "ranges": "3", "out_dir": dir.path() });

    assert_eq!(
        failure_kind(&split_by_ranges(&request.to_string())),
        "PageOutOfRange"
    );
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn a_request_that_is_not_the_operations_json_is_bad() {
    assert_eq!(failure_kind(&merge_files("not json")), "BadRequest");
    let unknown_field = json!({ "path": sample(), "pages": [1], "out": "x", "extra": 1 });
    assert_eq!(
        failure_kind(&extract_pages(&unknown_field.to_string())),
        "BadRequest"
    );
}

#[test]
fn a_file_with_a_rebuilt_cross_reference_compacts_to_a_new_file_that_opens_for_editing() {
    let dir = tempfile::tempdir().unwrap();
    let source = rebuilt_sample(dir.path());
    let before = fs::read(&source).unwrap();
    let out = dir.path().join("repaired.pdf");

    let refused = Bridge::new().open(&json!({ "path": source }).to_string());
    assert_eq!(failure_kind(&refused), "RebuiltNotEditable");

    let request = json!({ "path": source, "out": out });
    let answer = parse(&compact_file(&request.to_string()));

    assert_eq!(answer["ok"], Value::Bool(true));
    assert_eq!(answer["path"], json!(out));
    assert_eq!(pages_of(&out), SAMPLE_PAGES);
    assert_eq!(fs::read(&source).unwrap(), before);
    let engine = Bridge::new();
    let opened = parse(&engine.open(&json!({ "path": out }).to_string()));
    assert_eq!(opened["ok"], Value::Bool(true), "{opened}");
}

#[test]
fn compact_of_a_file_never_replaces_a_file_that_is_there() {
    let dir = tempfile::tempdir().unwrap();
    let source = rebuilt_sample(dir.path());
    let out = dir.path().join("repaired.pdf");
    fs::write(&out, b"the user's file").unwrap();
    let request = json!({ "path": source, "out": out });

    let kind = failure_kind(&compact_file(&request.to_string()));

    assert_eq!(kind, "Exists");
    assert_eq!(fs::read(&out).unwrap(), b"the user's file");
}

#[test]
fn compact_of_a_file_that_is_not_there_makes_no_file() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("repaired.pdf");
    let request = json!({ "path": dir.path().join("absent.pdf"), "out": out });

    assert_eq!(
        failure_kind(&compact_file(&request.to_string())),
        "NotFound"
    );
    assert!(!out.exists());
}
