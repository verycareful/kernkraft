// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The extractor an [`Engine`] keeps for each snapshot of a session: one per
//! revision, shared by the reads of that revision.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use kernkraft_pdf::doc::OpenOptions;
use kernkraft_pdf::file::ReadLimits;
use kernkraft_pdf_engine::edit::{ApplyRequest, Op};
use kernkraft_pdf_engine::open::Source;
use kernkraft_pdf_engine::read::SearchRequest;
use kernkraft_pdf_engine::{Engine, HandleRequest, PageRequest};

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

fn sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/kern-sample.pdf")
}

fn open(engine: &Engine) -> i64 {
    let source = Source {
        path: sample(),
        password: String::new(),
    };
    engine.open(source).unwrap()["handle"].as_i64().unwrap()
}

fn rotate(engine: &Engine, handle: i64) {
    engine
        .apply(ApplyRequest {
            handle,
            op: Op::Rotate {
                pages: vec![1],
                degrees: 90,
            },
        })
        .unwrap();
}

#[test]
fn reads_of_one_revision_share_one_extractor() {
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine);
    let (_, revision, first) = engine.text_extractor(handle).unwrap();
    engine.text(PageRequest { handle, page: 1 }).unwrap();
    engine
        .search(SearchRequest {
            handle,
            page: 1,
            query: "a".to_owned(),
            case_sensitive: false,
            whole_word: false,
        })
        .unwrap();
    let (_, again, second) = engine.text_extractor(handle).unwrap();
    assert_eq!(revision, again);
    assert!(Arc::ptr_eq(&first, &second));
}

#[test]
fn a_step_gives_the_next_read_a_new_extractor() {
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine);
    let (_, before, first) = engine.text_extractor(handle).unwrap();
    rotate(&engine, handle);
    let (_, after, second) = engine.text_extractor(handle).unwrap();
    assert_eq!(after, before + 1);
    assert!(!Arc::ptr_eq(&first, &second));
}

#[test]
fn an_undo_and_a_redo_each_give_a_new_extractor() {
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine);
    rotate(&engine, handle);
    let (_, _, edited) = engine.text_extractor(handle).unwrap();
    engine.undo(HandleRequest { handle }).unwrap();
    let (_, undone_revision, undone) = engine.text_extractor(handle).unwrap();
    assert!(!Arc::ptr_eq(&edited, &undone));
    engine.redo(HandleRequest { handle }).unwrap();
    let (_, redone_revision, redone) = engine.text_extractor(handle).unwrap();
    assert!(redone_revision > undone_revision);
    assert!(!Arc::ptr_eq(&undone, &redone));
}

#[test]
fn two_sessions_do_not_share_an_extractor() {
    let engine = Engine::new(OPTIONS);
    let one = open(&engine);
    let other = open(&engine);
    let (_, _, a) = engine.text_extractor(one).unwrap();
    let (_, _, b) = engine.text_extractor(other).unwrap();
    assert!(!Arc::ptr_eq(&a, &b));
}

#[test]
fn the_text_read_through_the_kept_extractor_is_the_same_each_time() {
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine);
    let first = engine.text(PageRequest { handle, page: 1 }).unwrap();
    let second = engine.text(PageRequest { handle, page: 1 }).unwrap();
    assert_eq!(first, second);
    assert!(!first["lines"].as_array().unwrap().is_empty());
}

#[test]
fn an_unknown_handle_is_refused_before_anything_is_built() {
    let engine = Engine::new(OPTIONS);
    let error = engine.text_extractor(99).err().unwrap();
    assert_eq!(error.kind, "UnknownHandle");
}
