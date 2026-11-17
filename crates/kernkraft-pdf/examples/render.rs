// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Draws one page of a PDF to a binary PPM, for scripts/check-render.sh.
//!
//!     render INPUT OUTPUT.ppm PAGE DPI [FONT_DIR...]
//!
//! The font directories are searched for `.ttf`, `.otf` and `.ttc` files,
//! which stand in for fonts a PDF does not embed. Exits with 0 when the
//! page is written, and 1 on any failure. The kinds of content the renderer
//! does not draw yet are listed on standard output, one `not-drawn FEATURE`
//! line each.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use kernkraft_font::FontList;
use kernkraft_fs::NoProgress;
use kernkraft_pdf::doc::{Document, OpenOptions};
use kernkraft_pdf::file::ReadLimits;
use kernkraft_pdf::render::{render_page, RenderOptions};
use kernkraft_pdf::DiagnosticKind;

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
const POINTS_PER_INCH: f64 = 72.0;
const FONT_EXTENSIONS: [&str; 3] = ["ttf", "otf", "ttc"];

fn font_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            font_files(&path, out);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| FONT_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        {
            out.push(path);
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let [input, output, page, dpi, fonts @ ..] = args else {
        return Err("usage: render INPUT OUTPUT.ppm PAGE DPI [FONT_DIR...]".into());
    };
    let page: u32 = page.parse().map_err(|_| "PAGE is not a number")?;
    let dpi: f64 = dpi.parse().map_err(|_| "DPI is not a number")?;
    let doc = Document::open(Path::new(input), &OPTIONS, "").map_err(|e| e.to_string())?;
    let mut paths = Vec::new();
    for dir in fonts {
        font_files(Path::new(dir), &mut paths);
    }
    let list = FontList::index(&paths, &NoProgress).map_err(|e| e.to_string())?;
    let options = RenderOptions {
        scale: dpi / POINTS_PER_INCH,
        fonts: Some(&list),
        ..RenderOptions::default()
    };
    let raster = render_page(&doc, page, &options, &NoProgress).map_err(|e| e.to_string())?;
    let mut left_out: Vec<&str> = doc
        .diagnostics()
        .iter()
        .filter_map(|d| match &d.kind {
            DiagnosticKind::NotRendered { feature } => Some(*feature),
            _ => None,
        })
        .collect();
    left_out.sort_unstable();
    left_out.dedup();
    for feature in left_out {
        println!("not-drawn {feature}");
    }
    let mut file = std::fs::File::create(output).map_err(|e| e.to_string())?;
    write!(file, "P6\n{} {}\n255\n", raster.width, raster.height).map_err(|e| e.to_string())?;
    let rgb: Vec<u8> = raster
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    file.write_all(&rgb).map_err(|e| e.to_string())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}
