// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The operations that open a file, make new ones and keep nothing: merge,
//! split by ranges and extract.
//!
//! Each output is a new file made through `kernkraft-fs`'s atomic writer
//! with `Expect::Nothing`: it is created, never replaces anything, and a
//! path that is already taken is `Exists`. A split that fails part way
//! removes the files it made.

use std::fs;
use std::path::{Path, PathBuf};

use kernkraft_fs::{Expect, NoProgress};
use kernkraft_pdf::doc::{Document, OpenOptions};
use kernkraft_pdf::write::save_compact;
use kernkraft_pdf::{extract, merge, parse_ranges, split_ranges};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::WireError;
use crate::fields::{field, Fields};
use crate::open::{write_options, Source};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MergeRequest {
    pub sources: Vec<Source>,
    pub out: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtractRequest {
    #[serde(flatten)]
    pub source: Source,
    /// 1-based page numbers, in the order the new file shows them.
    pub pages: Vec<u32>,
    pub out: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplitRequest {
    #[serde(flatten)]
    pub source: Source,
    /// Pages and ranges, `1-3,5,7-9`.
    pub ranges: String,
    pub out_dir: String,
}

/// Concatenates the sources, in order, into the new file `out`.
pub fn merge_files(options: &OpenOptions, request: MergeRequest) -> Result<Fields, WireError> {
    let documents = request
        .sources
        .iter()
        .map(|source| source.open(options))
        .collect::<Result<Vec<Document>, _>>()?;
    let borrowed: Vec<&Document> = documents.iter().collect();
    let merged = merge(&borrowed, options, &NoProgress)?;
    save_new(&merged, Path::new(&request.out))?;
    Ok(field("paths", json!([request.out])))
}

/// The listed pages of the source as the new file `out`.
pub fn extract_pages(options: &OpenOptions, request: ExtractRequest) -> Result<Fields, WireError> {
    let document = request.source.open(options)?;
    let pages = extract(&document, &request.pages)?;
    save_new(&pages, Path::new(&request.out))?;
    Ok(field("paths", json!([request.out])))
}

/// One new file per range, in `out_dir`, named after the source:
/// `<stem>_<first>-<last>.pdf`, or `<stem>_<page>.pdf` for a single page.
pub fn split_by_ranges(options: &OpenOptions, request: SplitRequest) -> Result<Fields, WireError> {
    let document = request.source.open(options)?;
    let ranges = parse_ranges(&request.ranges, document.page_count()?)?;
    let parts = split_ranges(&document, &ranges)?;

    let stem = Path::new(&request.source.path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| WireError::bad_request("the source path has no file name"))?;
    let dir = Path::new(&request.out_dir);
    let names: Vec<PathBuf> = ranges
        .iter()
        .map(|range| {
            let part = if range.start == range.end {
                format!("{stem}_{}.pdf", range.start)
            } else {
                format!("{stem}_{}-{}.pdf", range.start, range.end)
            };
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
        if let Err(error) = save_new(part, name) {
            for path in made {
                // Best effort: the file is one this call just made, and the
                // error being reported is the one that stopped the split.
                let _ = fs::remove_file(path);
            }
            return Err(error);
        }
        made.push(name);
    }
    let paths: Vec<Value> = names
        .iter()
        .map(|name| Value::String(name.to_string_lossy().into_owned()))
        .collect();
    Ok(field("paths", Value::Array(paths)))
}

fn save_new(document: &Document, path: &Path) -> Result<(), WireError> {
    save_compact(
        document,
        path,
        Expect::Nothing,
        &write_options()?,
        &NoProgress,
    )?;
    Ok(())
}

fn first_repeat(names: &[PathBuf]) -> Option<&PathBuf> {
    names
        .iter()
        .enumerate()
        .find(|(index, name)| names[..*index].contains(name))
        .map(|(_, name)| name)
}
