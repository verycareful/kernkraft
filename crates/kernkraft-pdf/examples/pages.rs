// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Runs a page operation and writes the result as a compacted copy, for
//! scripts/check-page-ops.sh.
//!
//!     pages extract INPUT OUTPUT PAGES
//!     pages remove  INPUT OUTPUT PAGES
//!     pages reorder INPUT OUTPUT ORDER
//!     pages rotate  INPUT OUTPUT PAGES DEGREES
//!     pages merge   OUTPUT INPUT INPUT...
//!
//! PAGES and ORDER are comma-separated page numbers. The inputs are opened
//! with the empty password. The seed is fixed, so the same input gives the
//! same output.

use std::path::Path;
use std::process::ExitCode;

use kernkraft_fs::{Expect, NoProgress};
use kernkraft_pdf::doc::{Document, OpenOptions};
use kernkraft_pdf::file::ReadLimits;
use kernkraft_pdf::write::{save_compact, WriteOptions};
use kernkraft_pdf::{extract, merge, remove, reorder, rotate, PdfError, Rotation};

const OPTIONS: OpenOptions = OpenOptions {
    limits: ReadLimits {
        max_object_elements: 4 << 20,
        max_xref_entries: 64 << 20,
        max_object_stream_bytes: 256 << 20,
    },
    cached_objects: 4096,
    cached_object_streams: 16,
    max_diagnostics: 10_000,
};
const SEED: WriteOptions = WriteOptions { seed: [0x5A; 32] };

fn open(path: &str) -> Result<Document, PdfError> {
    Document::open(Path::new(path), &OPTIONS, "")
}

fn numbers(list: &str) -> Result<Vec<u32>, String> {
    list.split(',')
        .map(|page| {
            page.trim()
                .parse()
                .map_err(|_| format!("not a page: {page}"))
        })
        .collect()
}

fn run(args: &[String]) -> Result<(), String> {
    let usage = || "usage: pages OPERATION ... (see the file header)".to_string();
    let operation = args.get(1).ok_or_else(usage)?;
    let error = |e: PdfError| e.to_string();
    let result = match operation.as_str() {
        "merge" => {
            let output = args.get(2).ok_or_else(usage)?;
            let documents = args[3..]
                .iter()
                .map(|path| open(path))
                .collect::<Result<Vec<_>, _>>()
                .map_err(error)?;
            let borrowed: Vec<&Document> = documents.iter().collect();
            let merged = merge(&borrowed, &OPTIONS, &NoProgress).map_err(error)?;
            (merged, output)
        }
        "extract" | "remove" | "reorder" | "rotate" => {
            let (input, output, list) = (
                args.get(2).ok_or_else(usage)?,
                args.get(3).ok_or_else(usage)?,
                args.get(4).ok_or_else(usage)?,
            );
            let document = open(input).map_err(error)?;
            let pages = numbers(list)?;
            let changed = match operation.as_str() {
                "extract" => extract(&document, &pages),
                "remove" => remove(&document, &pages),
                "reorder" => reorder(&document, &pages),
                _ => {
                    let by = match args.get(5).map(String::as_str) {
                        Some("90") => Rotation::Cw90,
                        Some("180") => Rotation::Cw180,
                        Some("270") => Rotation::Cw270,
                        _ => return Err("DEGREES is 90, 180 or 270".to_string()),
                    };
                    rotate(&document, &pages, by)
                }
            }
            .map_err(error)?;
            (changed, output)
        }
        _ => return Err(usage()),
    };
    let (document, output) = result;
    save_compact(
        &document,
        Path::new(output),
        Expect::Anything,
        &SEED,
        &NoProgress,
    )
    .map_err(error)?;
    for diagnostic in document.diagnostics() {
        eprintln!("diagnostic: {diagnostic:?}");
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}
