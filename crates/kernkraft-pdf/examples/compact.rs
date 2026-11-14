// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Writes a compacted copy of a PDF, for scripts/check-interop.sh.
//!
//!     compact INPUT OUTPUT [PASSWORD]
//!
//! Exits with 0 when the copy is written, 3 when the password does not
//! open the file, and 1 on any other failure. The seed is fixed, so the
//! same input gives the same output; a copy meant for use takes fresh
//! random bytes instead.

use std::path::Path;
use std::process::ExitCode;

use kernkraft_fs::{Expect, NoProgress};
use kernkraft_pdf::doc::{Document, OpenOptions};
use kernkraft_pdf::file::ReadLimits;
use kernkraft_pdf::write::{save_compact, WriteOptions};
use kernkraft_pdf::PdfError;

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
const WRONG_PASSWORD: u8 = 3;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let (Some(input), Some(output)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: compact INPUT OUTPUT [PASSWORD]");
        return ExitCode::FAILURE;
    };
    let password = args.get(3).map_or("", String::as_str);
    let result = Document::open(Path::new(input), &OPTIONS, password).and_then(|document| {
        let stamp = save_compact(
            &document,
            Path::new(output),
            Expect::Anything,
            &SEED,
            &NoProgress,
        );
        for diagnostic in document.diagnostics() {
            eprintln!("diagnostic: {diagnostic:?}");
        }
        stamp
    });
    match result {
        Ok(_) => ExitCode::SUCCESS,
        Err(PdfError::WrongPassword) => ExitCode::from(WRONG_PASSWORD),
        Err(error) => {
            eprintln!("{input}: {error}");
            ExitCode::FAILURE
        }
    }
}
