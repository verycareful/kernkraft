// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Opening, checking and saving text files through the plugin's methods.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;

use common::{env, w};

#[test]
fn text_open_save_round_trip_crlf() {
    let e = env();
    let path = e.dir("t").join("notes.txt");
    fs::write(&path, b"one\r\ntwo\r\n").unwrap();

    let opened = e.kk.text_open(w(&path)).unwrap();
    // The editor sees LF only; the file's CRLF is remembered.
    assert_eq!(opened.text, "one\ntwo\n");
    assert_eq!(opened.ending, "crlf");
    assert!(!opened.bom);
    assert_eq!(opened.read_only, None);
    assert_eq!(opened.path, w(&path));

    let saved =
        e.kk.text_save(
            w(&path),
            "one\ntwo\nthree\n".to_owned(),
            opened.ending.clone(),
            opened.bom,
            Some(opened.stamp.clone()),
        )
        .unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"one\r\ntwo\r\nthree\r\n");

    // The returned stamp is the file now on disk.
    let again = e.kk.text_open(w(&path)).unwrap();
    assert_eq!(again.stamp, saved);
    assert_eq!(e.kk.text_check(w(&path), saved).unwrap(), "same");
}

#[test]
fn text_open_keeps_bom_and_marks_read_only_reasons() {
    let e = env();
    let dir = e.dir("t");
    let bom = dir.join("bom.txt");
    fs::write(&bom, b"\xEF\xBB\xBFhi\n").unwrap();
    let opened = e.kk.text_open(w(&bom)).unwrap();
    assert!(opened.bom);
    assert_eq!(opened.text, "hi\n");
    assert_eq!(opened.ending, "lf");

    let latin1 = dir.join("latin1.txt");
    // 0xE9 alone is not UTF-8 (RFC 3629 section 4): byte 2 is the first
    // invalid sequence.
    fs::write(&latin1, b"ab\xE9\n").unwrap();
    let ro = e.kk.text_open(w(&latin1)).unwrap().read_only.unwrap();
    assert_eq!(ro.kind, "NotUtf8");
    assert!(ro.detail.contains('2'), "{}", ro.detail);
}

#[test]
fn text_save_stale_stamp_errors() {
    let e = env();
    let path = e.dir("t").join("a.txt");
    fs::write(&path, "mine\n").unwrap();
    let opened = e.kk.text_open(w(&path)).unwrap();

    // Another program writes the file after it was opened.
    fs::write(&path, "theirs, longer\n").unwrap();
    assert_eq!(
        e.kk.text_check(w(&path), opened.stamp.clone()).unwrap(),
        "changed"
    );
    let err =
        e.kk.text_save(
            w(&path),
            "mine, edited\n".to_owned(),
            "lf".to_owned(),
            false,
            Some(opened.stamp.clone()),
        )
        .unwrap_err();
    assert_eq!(err.kind, "ExternalModification");
    assert_eq!(fs::read_to_string(&path).unwrap(), "theirs, longer\n");

    // Saving without a stamp is the user's explicit overwrite.
    e.kk.text_save(
        w(&path),
        "mine, edited\n".to_owned(),
        "lf".to_owned(),
        false,
        None,
    )
    .unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "mine, edited\n");

    fs::remove_file(&path).unwrap();
    assert_eq!(e.kk.text_check(w(&path), opened.stamp).unwrap(), "deleted");
}

#[test]
fn text_save_refuses_unknown_ending_and_open_refuses_large() {
    let e = env();
    let path = e.dir("t").join("a.txt");
    let err =
        e.kk.text_save(w(&path), String::new(), "cr".to_owned(), false, None)
            .unwrap_err();
    assert_eq!(err.kind, "InvalidArgument");
    assert!(!path.exists());

    let big = e.dir("t").join("big.txt");
    let f = fs::File::create(&big).unwrap();
    // One byte over the open limit, as a sparse file.
    f.set_len(kernkraft_text::OPEN_LIMIT + 1).unwrap();
    let err = e.kk.text_open(w(&big)).unwrap_err();
    assert_eq!(err.kind, "TooLargeToOpen");
}

#[test]
fn text_check_refuses_a_forged_stamp() {
    let e = env();
    let path = e.dir("t").join("a.txt");
    fs::write(&path, "x").unwrap();
    let forged: kernkraft_tauri::WireStamp = serde_json::from_str("\"not a stamp\"").unwrap();
    let err = e.kk.text_check(w(&path), forged).unwrap_err();
    assert_eq!(err.kind, "InvalidArgument");
}
