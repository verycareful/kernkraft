// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The operations that open a file, make new ones and keep nothing: merge,
//! split by ranges, extract and compact a file.
//!
//! Each output is a new file made through `kernkraft-fs`'s atomic writer
//! with `Expect::Nothing`: it is created, never replaces anything, and a
//! path that is already taken is `Exists`. A split that fails part way
//! removes the files it made.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use kernkraft_fs::{Expect, Progress};
use kernkraft_pdf::doc::Document;
use kernkraft_pdf::write::save_compact;
use kernkraft_pdf::{extract, merge, parse_ranges, split_ranges};
use serde::Deserialize;

use crate::engine::Engine;
use crate::error::WireError;
use crate::jobs::JobId;
use crate::open::{write_options, Source};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MergeRequest {
    pub sources: Vec<Source>,
    pub out: PathBuf,
    /// The job that can cancel this request and watch it (see
    /// [`crate::jobs`]); `done` counts sources.
    #[serde(default)]
    pub job: Option<JobId>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtractRequest {
    #[serde(flatten)]
    pub source: Source,
    /// 1-based page numbers, in the order the new file shows them.
    pub pages: Vec<u32>,
    pub out: PathBuf,
    /// The job that can cancel this request and watch it (see
    /// [`crate::jobs`]). Taking the pages out of the file does not poll the
    /// job; the write of the new file does.
    #[serde(default)]
    pub job: Option<JobId>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactFileRequest {
    #[serde(flatten)]
    pub source: Source,
    pub out: PathBuf,
    /// The job that can cancel this request and watch it (see
    /// [`crate::jobs`]); `done` counts objects.
    #[serde(default)]
    pub job: Option<JobId>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplitRequest {
    #[serde(flatten)]
    pub source: Source,
    /// Pages and ranges, `1-3,5,7-9`.
    pub ranges: String,
    pub out_dir: PathBuf,
    /// The job that can cancel this request and watch it (see
    /// [`crate::jobs`]). Taking the pages out of the file does not poll the
    /// job; the write of each new file does.
    #[serde(default)]
    pub job: Option<JobId>,
}

/// Concatenates the sources, in order, into the new file `out`, which is
/// the one path given back.
pub fn merge_files(engine: &Engine, request: MergeRequest) -> Result<Vec<PathBuf>, WireError> {
    let options = engine.options();
    let running = engine.jobs.begin(request.job, None)?;
    let documents = request
        .sources
        .iter()
        .map(|source| source.open(options))
        .collect::<Result<Vec<Document>, _>>()?;
    let borrowed: Vec<&Document> = documents.iter().collect();
    let merged = merge(&borrowed, options, &running)?;
    save_new(&merged, &request.out, &running)?;
    Ok(vec![request.out])
}

/// The listed pages of the source as the new file `out`, which is the one
/// path given back.
pub fn extract_pages(engine: &Engine, request: ExtractRequest) -> Result<Vec<PathBuf>, WireError> {
    let running = engine.jobs.begin(request.job, None)?;
    let document = request.source.open(engine.options())?;
    let pages = extract(&document, &request.pages)?;
    save_new(&pages, &request.out, &running)?;
    Ok(vec![request.out])
}

/// The source written as the new, compacted file `out`, which is given
/// back. The source is read and never changed, and it need not open for
/// editing: a file whose cross-reference was rebuilt is written out whole,
/// with a cross-reference that is not rebuilt.
pub fn compact_file(engine: &Engine, request: CompactFileRequest) -> Result<PathBuf, WireError> {
    let running = engine.jobs.begin(request.job, None)?;
    let document = request.source.open(engine.options())?;
    save_new(&document, &request.out, &running)?;
    Ok(request.out)
}

/// One new file per range, in `out_dir`, named after the source:
/// `<stem>_<first>-<last>.pdf`, or `<stem>_<page>.pdf` for a single page.
/// The files made are given back, in the order of the ranges.
pub fn split_by_ranges(engine: &Engine, request: SplitRequest) -> Result<Vec<PathBuf>, WireError> {
    let running = engine.jobs.begin(request.job, None)?;
    let document = request.source.open(engine.options())?;
    let ranges = parse_ranges(&request.ranges, document.page_count()?)?;
    let parts = split_ranges(&document, &ranges)?;

    let stem = request
        .source
        .path
        .file_stem()
        .ok_or_else(|| WireError::bad_request("the source path has no file name"))?;
    let dir = &request.out_dir;
    let names: Vec<PathBuf> = ranges
        .iter()
        .map(|range| {
            let mut part: OsString = stem.to_os_string();
            if range.start == range.end {
                part.push(format!("_{}.pdf", range.start));
            } else {
                part.push(format!("_{}-{}.pdf", range.start, range.end));
            }
            dir.join(part)
        })
        .collect();
    if let Some(name) = first_repeat(&names) {
        return Err(WireError::bad_request(format!(
            "two ranges give the file {}",
            name.display()
        )));
    }

    let mut made: Vec<&PathBuf> = Vec::new();
    for (part, name) in parts.iter().zip(&names) {
        if let Err(error) = save_new(part, name, &running) {
            for path in made {
                // Best effort: the file is one this call just made, and the
                // error being reported is the one that stopped the split.
                let _ = fs::remove_file(path);
            }
            return Err(error);
        }
        made.push(name);
    }
    Ok(names)
}

fn save_new(document: &Document, path: &Path, progress: &dyn Progress) -> Result<(), WireError> {
    save_compact(document, path, Expect::Nothing, &write_options()?, progress)?;
    Ok(())
}

fn first_repeat(names: &[PathBuf]) -> Option<&PathBuf> {
    names
        .iter()
        .enumerate()
        .find(|(index, name)| names[..*index].contains(name))
        .map(|(_, name)| name)
}
