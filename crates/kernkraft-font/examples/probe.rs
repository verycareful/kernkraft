// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Indexes the font files named in a list (one path per line) and opens each
//! one as a program: `probe LIST`. Prints the index time, the files skipped
//! with their reasons, and what opening found. For checking the crate
//! against real fonts.

use std::path::PathBuf;
use std::time::Instant;

use kernkraft_font::{FontList, GlyphId, Program, Query};
use kernkraft_fs::NoProgress;

fn main() {
    let Some(list_path) = std::env::args().nth(1) else {
        eprintln!("usage: probe LIST");
        std::process::exit(2);
    };
    let text = match std::fs::read_to_string(&list_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("cannot read {list_path}: {e}");
            std::process::exit(1);
        }
    };
    let paths: Vec<PathBuf> = text
        .lines()
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect();

    let start = Instant::now();
    let list = match FontList::index(&paths, &NoProgress) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("index failed: {e}");
            std::process::exit(1);
        }
    };
    println!(
        "index: {} files, {} faces, {} skipped, {:?}",
        paths.len(),
        list.faces().len(),
        list.skipped().len(),
        start.elapsed()
    );
    for s in list.skipped() {
        println!("skipped {}: {}", s.path.display(), s.error);
    }

    let mut opened = 0usize;
    let mut failed = 0usize;
    let mut bad_outlines = 0usize;
    let mut no_outlines = 0usize;
    let mut glyphs_checked = 0usize;
    let start = Instant::now();
    for face in list.faces() {
        let data = match std::fs::read(&face.path) {
            Ok(d) => d,
            Err(e) => {
                println!("read {}: {e}", face.path.display());
                failed += 1;
                continue;
            }
        };
        match Program::parse(&data, face.index) {
            Ok(p) => {
                opened += 1;
                // Every 97th glyph, so a large font costs a bounded time.
                for g in (0..p.glyph_count()).step_by(97) {
                    glyphs_checked += 1;
                    match p.outline(GlyphId(g)) {
                        Ok(_) => {}
                        Err(kernkraft_font::FontError::NoOutline(_)) => no_outlines += 1,
                        Err(e) => {
                            bad_outlines += 1;
                            println!(
                                "outline {} #{} glyph {g}: {e}",
                                face.path.display(),
                                face.index
                            );
                        }
                    }
                }
            }
            Err(e) => {
                failed += 1;
                println!("open {} #{}: {e}", face.path.display(), face.index);
            }
        }
    }
    println!("open: {opened} opened, {failed} failed, {glyphs_checked} glyphs sampled, {no_outlines} with no vector outline, {bad_outlines} bad, {:?}", start.elapsed());

    for family in ["DejaVu Sans", "Noto Sans CJK JP", "Liberation Serif"] {
        let fam = [family];
        let q = Query {
            families: &fam,
            weight: 700,
            ..Query::default()
        };
        match list.best(&q) {
            Some(f) => println!(
                "best {family} 700: {} #{} weight {} italic {}",
                f.path.display(),
                f.index,
                f.weight,
                f.italic
            ),
            None => println!("best {family} 700: none"),
        }
    }
    let q = Query::default();
    for c in ['A', '\u{3042}', '\u{0905}', '\u{1F600}'] {
        match list.covering(c, &q) {
            Some(f) => println!(
                "covering U+{:04X}: {} #{}",
                c as u32,
                f.path.display(),
                f.index
            ),
            None => println!("covering U+{:04X}: none", c as u32),
        }
    }
}
