// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Jobs: a long request that names a job stops with `Cancelled` when the job
//! is cancelled, and leaves nothing behind.
//!
//! A cancel that arrives before its request has begun is kept and applies as
//! the request begins, so each case cancels first and then makes the request:
//! the outcome does not depend on how fast the request runs. Each
//! expectation follows from the operation: a cancelled save writes no file,
//! a cancelled edit leaves the revision where it was, a cancelled font index
//! leaves the fonts it had.

use std::fs;
use std::path::{Path, PathBuf};

use kernkraft_fs::Expect;
use kernkraft_pdf::doc::OpenOptions;
use kernkraft_pdf::file::ReadLimits;
use kernkraft_pdf_engine::edit::{ApplyRequest, Op};
use kernkraft_pdf_engine::fonts::FontsRequest;
use kernkraft_pdf_engine::jobs::JobRequest;
use kernkraft_pdf_engine::open::Source;
use kernkraft_pdf_engine::read::{SearchRequest, TextRequest};
use kernkraft_pdf_engine::render::RenderRequest;
use kernkraft_pdf_engine::save::{AppendRequest, CompactRequest};
use kernkraft_pdf_engine::stateless::{
    self, CompactFileRequest, ExtractRequest, MergeRequest, SplitRequest,
};
use kernkraft_pdf_engine::Engine;

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

const DATE: &str = "D:20261121103000Z";

fn sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/kern-sample.pdf")
}

fn source() -> Source {
    Source {
        path: sample(),
        password: String::new(),
    }
}

fn open(engine: &Engine) -> i64 {
    engine.open(source()).unwrap()["handle"].as_i64().unwrap()
}

/// Asks for job `job` to stop before any request has it.
fn cancel(engine: &Engine, job: u64) {
    assert_eq!(engine.cancel_job(JobRequest { job })["running"], false);
}

fn rotate(engine: &Engine, handle: i64) {
    engine
        .apply(ApplyRequest {
            handle,
            op: Op::Rotate {
                pages: vec![1],
                degrees: 90,
            },
            job: None,
        })
        .unwrap();
}

#[test]
fn a_cancelled_save_writes_nothing_and_the_job_number_is_then_free() {
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine);
    rotate(&engine, handle);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("saved.pdf");
    cancel(&engine, 7);

    let stopped = engine
        .save(handle, &out, Expect::Nothing, DATE, Some(7))
        .unwrap_err();
    assert_eq!(stopped.kind, "Cancelled");
    assert!(!out.exists());

    // The cancel was for one job: the same number, asked again, runs.
    engine
        .save(handle, &out, Expect::Nothing, DATE, Some(7))
        .unwrap();
    assert!(out.exists());
}

#[test]
fn a_save_with_a_job_that_is_not_cancelled_is_the_save_without_one() {
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine);
    rotate(&engine, handle);
    let dir = tempfile::tempdir().unwrap();
    let plain = dir.path().join("plain.pdf");
    let named = dir.path().join("named.pdf");

    engine
        .save(handle, &plain, Expect::Nothing, DATE, None)
        .unwrap();
    engine
        .save(handle, &named, Expect::Nothing, DATE, Some(1))
        .unwrap();

    // The same update on the same opened bytes: the file's identifier is the
    // one thing that differs between two saves. It is 16 bytes written as a
    // literal string, in which a parenthesis, a backslash or a carriage
    // return takes two bytes, so the two files differ in length by at most
    // 16.
    const ID_BYTES: usize = 16;
    let original = fs::read(sample()).unwrap();
    let (plain, named) = (fs::read(&plain).unwrap(), fs::read(&named).unwrap());
    assert!(plain.starts_with(&original) && named.starts_with(&original));
    assert!(plain.len().abs_diff(named.len()) <= ID_BYTES);
    // Over, the job is not running and has nothing counted against it.
    let progress = engine.job_progress(JobRequest { job: 1 });
    assert_eq!(progress["running"], false);
    assert_eq!(progress["done"], 0);
}

#[test]
fn a_cancelled_append_and_a_cancelled_compact_write_nothing() {
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine);
    rotate(&engine, handle);
    let dir = tempfile::tempdir().unwrap();
    let appended = dir.path().join("append.bin");
    let compacted = dir.path().join("compact.pdf");
    cancel(&engine, 1);
    cancel(&engine, 2);

    let append = engine
        .append_since(AppendRequest {
            handle,
            base_length: fs::metadata(sample()).unwrap().len(),
            out: appended.clone(),
            modified: DATE.to_owned(),
            job: Some(1),
        })
        .unwrap_err();
    let compact = engine
        .compact(CompactRequest {
            handle,
            out: compacted.clone(),
            job: Some(2),
        })
        .unwrap_err();

    assert_eq!(append.kind, "Cancelled");
    assert_eq!(compact.kind, "Cancelled");
    assert!(!appended.exists());
    assert!(!compacted.exists());
}

#[test]
fn a_cancelled_merge_makes_no_file() {
    let engine = Engine::new(OPTIONS);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("merged.pdf");
    cancel(&engine, 3);

    let stopped = stateless::merge_files(
        &engine,
        MergeRequest {
            sources: vec![source(), source()],
            out: out.clone(),
            job: Some(3),
        },
    )
    .unwrap_err();

    assert_eq!(stopped.kind, "Cancelled");
    assert!(!out.exists());
}

#[test]
fn a_cancelled_extract_and_a_cancelled_split_make_no_file() {
    let engine = Engine::new(OPTIONS);
    let dir = tempfile::tempdir().unwrap();
    let extracted = dir.path().join("page.pdf");
    cancel(&engine, 4);
    cancel(&engine, 5);

    let extract = stateless::extract_pages(
        &engine,
        ExtractRequest {
            source: source(),
            pages: vec![1],
            out: extracted.clone(),
            job: Some(4),
        },
    )
    .unwrap_err();
    let split = stateless::split_by_ranges(
        &engine,
        SplitRequest {
            source: source(),
            ranges: "1,2".to_owned(),
            out_dir: dir.path().to_path_buf(),
            job: Some(5),
        },
    )
    .unwrap_err();

    assert_eq!(extract.kind, "Cancelled");
    assert_eq!(split.kind, "Cancelled");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn a_cancelled_compact_of_a_file_makes_no_file() {
    let engine = Engine::new(OPTIONS);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("compacted.pdf");
    cancel(&engine, 6);

    let stopped = stateless::compact_file(
        &engine,
        CompactFileRequest {
            source: source(),
            out: out.clone(),
            job: Some(6),
        },
    )
    .unwrap_err();

    assert_eq!(stopped.kind, "Cancelled");
    assert!(!out.exists());
}

#[test]
fn a_cancelled_render_draws_nothing() {
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine);
    let request = |job| RenderRequest {
        handle,
        page: 1,
        region: None,
        width_px: 200,
        job,
    };
    let bytes = engine.render_size(request(None)).unwrap()["bytes"]
        .as_u64()
        .unwrap();
    let mut buffer = vec![0_u8; usize::try_from(bytes).unwrap()];
    cancel(&engine, 6);

    let stopped = engine.render(request(Some(6)), &mut buffer).unwrap_err();

    assert_eq!(stopped.kind, "Cancelled");
    assert!(buffer.iter().all(|byte| *byte == 0));
}

#[test]
fn a_cancelled_page_read_and_a_cancelled_search_answer_cancelled() {
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine);
    cancel(&engine, 8);
    cancel(&engine, 9);

    let text = engine
        .text(TextRequest {
            handle,
            page: 1,
            job: Some(8),
        })
        .unwrap_err();
    let search = engine
        .search(SearchRequest {
            handle,
            page: 1,
            query: "a".to_owned(),
            case_sensitive: false,
            whole_word: false,
            job: Some(9),
        })
        .unwrap_err();

    assert_eq!(text.kind, "Cancelled");
    assert_eq!(search.kind, "Cancelled");
}

#[test]
fn a_cancelled_text_edit_leaves_the_document_at_its_revision() {
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine);
    let before = engine
        .text(TextRequest {
            handle,
            page: 1,
            job: None,
        })
        .unwrap();
    cancel(&engine, 10);

    let stopped = engine
        .apply(ApplyRequest {
            handle,
            op: Op::EditText {
                page: 1,
                block: 0,
                revision: before["revision"].as_u64().unwrap(),
                text: "changed".to_owned(),
                family: None,
                size: None,
                colour: None,
                left: None,
                width: None,
            },
            job: Some(10),
        })
        .unwrap_err();

    assert_eq!(stopped.kind, "Cancelled");
    let after = engine
        .text(TextRequest {
            handle,
            page: 1,
            job: None,
        })
        .unwrap();
    assert_eq!(after["revision"], before["revision"]);
    assert_eq!(after["lines"], before["lines"]);
}

#[test]
fn a_cancelled_font_index_keeps_the_fonts_it_had() {
    let engine = Engine::new(OPTIONS);
    cancel(&engine, 11);

    let stopped = engine
        .set_system_fonts(FontsRequest {
            paths: vec![sample()],
            job: Some(11),
        })
        .unwrap_err();

    assert_eq!(stopped.kind, "Cancelled");
    assert!(engine.fonts().faces().is_empty());
}

#[test]
fn a_cancel_of_one_job_does_not_stop_another() {
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine);
    cancel(&engine, 12);

    let text = engine
        .text(TextRequest {
            handle,
            page: 1,
            job: Some(13),
        })
        .unwrap();

    assert!(text["blocks"].is_array());
}
