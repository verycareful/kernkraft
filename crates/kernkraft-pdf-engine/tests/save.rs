// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `Engine::save`: the opened bytes and one update, written atomically over
//! a path, and the paths an engine opens.

use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use kernkraft_fs::{Expect, FileStamp};
use kernkraft_pdf::doc::{Document, OpenOptions};
use kernkraft_pdf::file::ReadLimits;
use kernkraft_pdf_engine::edit::{ApplyRequest, Op};
use kernkraft_pdf_engine::open::Source;
use kernkraft_pdf_engine::{Engine, HandleRequest};

const OPTIONS: OpenOptions = OpenOptions {
    limits: ReadLimits {
        max_object_elements: 1 << 20,
        max_xref_entries: 1 << 24,
        max_object_stream_bytes: 1 << 26,
    },
    cached_objects: 64,
    cached_object_streams: 4,
    max_diagnostics: 100,
};

/// A time to stamp the saves with: a PDF date.
const DATE: &str = "D:20261121103000Z";

fn sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/kern-sample.pdf")
}

/// A copy of the sample in a new folder, so a test may replace it.
fn copy_of_sample() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("work.pdf");
    fs::copy(sample(), &path).unwrap();
    (dir, path)
}

fn open(engine: &Engine, path: &Path) -> i64 {
    let source = Source {
        path: path.to_path_buf(),
        password: String::new(),
    };
    engine.open(source).unwrap()["handle"].as_i64().unwrap()
}

fn rotate(engine: &Engine, handle: i64, page: u32) {
    let request = ApplyRequest {
        handle,
        op: Op::Rotate {
            pages: vec![page],
            degrees: 90,
        },
    };
    engine.apply(request).unwrap();
}

fn rotation_of(path: &Path, page: u32) -> u16 {
    Document::open(path, &OPTIONS, "")
        .unwrap()
        .page_geometry(page)
        .unwrap()
        .rotate
}

#[test]
fn a_save_over_the_opened_file_is_the_original_bytes_and_one_update() {
    let (_dir, path) = copy_of_sample();
    let original = fs::read(&path).unwrap();
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine, &path);
    rotate(&engine, handle, 1);
    let stamp = FileStamp::of(&path).unwrap();

    let saved = engine
        .save(handle, &path, Expect::Unchanged(&stamp), DATE)
        .unwrap();

    let written = fs::read(&path).unwrap();
    assert!(written.starts_with(&original));
    assert!(written.len() > original.len());
    assert_eq!(rotation_of(&path, 1), 90);
    assert_eq!(rotation_of(&path, 2), 0);
    assert_eq!(saved, FileStamp::of(&path).unwrap());
}

#[test]
fn a_second_save_is_made_against_the_opened_bytes_not_the_first_save() {
    let (_dir, path) = copy_of_sample();
    let original = fs::read(&path).unwrap();
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine, &path);
    rotate(&engine, handle, 1);
    let first = engine
        .save(
            handle,
            &path,
            Expect::Unchanged(&FileStamp::of(&path).unwrap()),
            DATE,
        )
        .unwrap();
    rotate(&engine, handle, 2);

    engine
        .save(handle, &path, Expect::Unchanged(&first), DATE)
        .unwrap();

    let written = fs::read(&path).unwrap();
    assert!(written.starts_with(&original));
    assert_eq!(rotation_of(&path, 1), 90);
    assert_eq!(rotation_of(&path, 2), 90);
    // One update after the original, not two: the first save's update is
    // not kept under the second's.
    let updates = written
        .windows(b"startxref".len())
        .filter(|w| *w == b"startxref")
        .count()
        - original
            .windows(b"startxref".len())
            .filter(|w| *w == b"startxref")
            .count();
    assert_eq!(updates, 1);
}

#[test]
fn a_file_changed_since_it_was_stamped_is_not_replaced() {
    let (_dir, path) = copy_of_sample();
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine, &path);
    rotate(&engine, handle, 1);
    let stamp = FileStamp::of(&path).unwrap();
    let mut changed = fs::read(&path).unwrap();
    changed.push(b'\n');
    fs::write(&path, &changed).unwrap();

    let refused = engine
        .save(handle, &path, Expect::Unchanged(&stamp), DATE)
        .unwrap_err();

    assert_eq!(refused.kind, "ExternalModification");
    assert_eq!(fs::read(&path).unwrap(), changed);
}

#[test]
fn a_save_expecting_nothing_does_not_replace_a_file() {
    let (dir, path) = copy_of_sample();
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine, &path);
    rotate(&engine, handle, 1);
    let taken = dir.path().join("taken.pdf");
    fs::write(&taken, b"mine").unwrap();

    let refused = engine
        .save(handle, &taken, Expect::Nothing, DATE)
        .unwrap_err();

    assert_eq!(refused.kind, "Exists");
    assert_eq!(fs::read(&taken).unwrap(), b"mine");
    let elsewhere = dir.path().join("copy.pdf");
    engine
        .save(handle, &elsewhere, Expect::Nothing, DATE)
        .unwrap();
    assert_eq!(rotation_of(&elsewhere, 1), 90);
}

#[test]
fn a_bad_date_and_an_unknown_handle_are_refused_before_anything_is_written() {
    let (dir, path) = copy_of_sample();
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine, &path);
    let out = dir.path().join("out.pdf");

    let date = engine
        .save(handle, &out, Expect::Nothing, "yesterday")
        .unwrap_err();
    let unknown = engine
        .save(handle + 1, &out, Expect::Nothing, DATE)
        .unwrap_err();

    assert_eq!(date.kind, "BadRequest");
    assert_eq!(unknown.kind, "UnknownHandle");
    assert!(!out.exists());
}

#[test]
fn a_file_name_that_is_not_utf8_opens() {
    let (dir, path) = copy_of_sample();
    let odd = dir.path().join(OsStr::from_bytes(b"caf\xE9.pdf"));
    fs::rename(&path, &odd).unwrap();
    let engine = Engine::new(OPTIONS);

    let handle = open(&engine, &odd);

    engine.close(HandleRequest { handle }).unwrap();
}
