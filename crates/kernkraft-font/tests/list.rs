// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The font list: indexing files, and choosing faces from it.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::{Path, PathBuf};

use common::sfnt::{self, Glyph, SfntSpec};
use common::{Cs, Type1Spec};
use kernkraft_font::{FontError, FontList, Query};
use kernkraft_fs::{NoProgress, Progress};

struct Face {
    family: &'static str,
    weight: u16,
    italic: bool,
    mono: bool,
    chars: &'static [char],
}

fn font(f: &Face) -> Vec<u8> {
    let unicode = f
        .chars
        .iter()
        .enumerate()
        .map(|(i, &c)| (c as u16, i as u16 + 1))
        .collect();
    let glyphs = std::iter::once(Glyph {
        name: ".notdef",
        advance: 500,
        contours: vec![],
    })
    .chain(f.chars.iter().map(|_| Glyph {
        name: "g",
        advance: 500,
        contours: sfnt::triangle(),
    }))
    .collect();
    sfnt::build(&SfntSpec {
        family: f.family,
        weight: f.weight,
        italic: f.italic,
        fixed_pitch: f.mono,
        unicode,
        ..SfntSpec::new(glyphs)
    })
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

fn query<'a>(families: &'a [&'a str], weight: u16, italic: bool) -> Query<'a> {
    Query {
        families,
        weight,
        italic,
        ..Query::default()
    }
}

fn names(faces: &[&kernkraft_font::FaceInfo]) -> Vec<(String, u16, bool)> {
    faces
        .iter()
        .map(|f| (f.families[0].clone(), f.weight, f.italic))
        .collect()
}

#[test]
fn a_face_is_indexed_by_name_style_pitch_and_coverage() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(
        dir.path(),
        "a.ttf",
        &font(&Face {
            family: "Test Mono",
            weight: 700,
            italic: true,
            mono: true,
            chars: &['A', 'B', ' '],
        }),
    );
    let list = FontList::index(std::slice::from_ref(&p), &NoProgress).unwrap();
    assert!(list.skipped().is_empty());
    let [face] = list.faces() else {
        panic!("one face expected")
    };
    assert_eq!(face.path, p);
    assert_eq!(face.index, 0);
    assert_eq!(face.families, vec!["Test Mono".to_string()]);
    assert_eq!(face.full_name.as_deref(), Some("Test Sans Regular"));
    assert_eq!(face.postscript_name.as_deref(), Some("TestSans-Regular"));
    assert_eq!(
        (face.weight, face.width, face.italic, face.monospaced),
        (700, 5, true, true)
    );
    // U+0020 and U+0041..=U+0042.
    assert_eq!(
        face.coverage.as_ref().unwrap().ranges(),
        &[(0x20, 0x20), (0x41, 0x42)]
    );
    assert!(face.covers('A') && face.covers(' ') && !face.covers('C'));
}

#[test]
fn the_glyph_tables_are_never_read() {
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = font(&Face {
        family: "Solid",
        weight: 400,
        italic: false,
        mono: false,
        chars: &['A'],
    });
    let (off, len) = sfnt::table(&bytes, b"glyf").unwrap();
    bytes[off..off + len].fill(0xFF);
    let (off, len) = sfnt::table(&bytes, b"hmtx").unwrap();
    bytes[off..off + len].fill(0xFF);
    let p = write(dir.path(), "a.ttf", &bytes);
    let list = FontList::index(&[p], &NoProgress).unwrap();
    assert_eq!(list.faces().len(), 1);
    assert!(list.skipped().is_empty());
}

#[test]
fn a_collection_gives_one_face_per_font() {
    let dir = tempfile::tempdir().unwrap();
    let a = font(&Face {
        family: "Duo One",
        weight: 400,
        italic: false,
        mono: false,
        chars: &['A'],
    });
    let b = font(&Face {
        family: "Duo Two",
        weight: 700,
        italic: false,
        mono: false,
        chars: &['B'],
    });
    let p = write(dir.path(), "duo.ttc", &sfnt::collection(&[a, b]));
    let list = FontList::index(&[p], &NoProgress).unwrap();
    let got: Vec<(u32, &str, u16)> = list
        .faces()
        .iter()
        .map(|f| (f.index, f.families[0].as_str(), f.weight))
        .collect();
    assert_eq!(got, vec![(0, "Duo One", 400), (1, "Duo Two", 700)]);
    assert!(list.faces()[0].covers('A') && !list.faces()[0].covers('B'));
    assert!(list.faces()[1].covers('B'));
}

#[test]
fn files_that_cannot_be_indexed_are_listed_with_their_reason() {
    let dir = tempfile::tempdir().unwrap();
    let good = write(
        dir.path(),
        "good.ttf",
        &font(&Face {
            family: "Good",
            weight: 400,
            italic: false,
            mono: false,
            chars: &['A'],
        }),
    );
    let text = write(dir.path(), "notes.txt", b"hello there");
    let empty = write(dir.path(), "empty.ttf", b"");
    let missing = dir.path().join("missing.ttf");
    let folder = dir.path().join("folder");
    std::fs::create_dir(&folder).unwrap();
    let full = font(&Face {
        family: "Cut",
        weight: 400,
        italic: false,
        mono: false,
        chars: &['A'],
    });
    let cut = write(dir.path(), "cut.ttf", &full[..40]);
    // A name table whose length runs past the end of the file.
    let mut long = full.clone();
    let at = (0..u16::from_be_bytes([long[4], long[5]]) as usize)
        .map(|i| 12 + i * 16)
        .find(|&r| &long[r..r + 4] == b"name")
        .unwrap();
    long[at + 12..at + 16].copy_from_slice(&u32::MAX.to_be_bytes());
    let past = write(dir.path(), "past.ttf", &long);

    let list = FontList::index(
        &[
            good,
            text.clone(),
            empty.clone(),
            missing.clone(),
            folder.clone(),
            cut.clone(),
            past.clone(),
        ],
        &NoProgress,
    )
    .unwrap();
    assert_eq!(list.faces().len(), 1);
    let why = |p: &PathBuf| {
        list.skipped()
            .iter()
            .find(|s| &s.path == p)
            .map(|s| s.error.clone())
            .unwrap()
    };
    assert_eq!(why(&text), FontError::UnknownFormat);
    assert_eq!(why(&empty), FontError::UnknownFormat);
    assert_eq!(why(&folder), FontError::UnknownFormat);
    assert_eq!(
        why(&missing),
        FontError::Io {
            kind: std::io::ErrorKind::NotFound
        }
    );
    assert!(matches!(why(&cut), FontError::Sfnt(_)));
    assert_eq!(
        why(&past),
        FontError::Sfnt("a table runs past the end of the file")
    );
    assert_eq!(list.skipped().len(), 6);
}

#[test]
fn the_same_files_give_the_same_list_in_any_order() {
    let dir = tempfile::tempdir().unwrap();
    let mut paths = Vec::new();
    for (i, family) in ["Aa", "Bb", "Cc"].into_iter().enumerate() {
        paths.push(write(
            dir.path(),
            &format!("{i}.ttf"),
            &font(&Face {
                family,
                weight: 400,
                italic: false,
                mono: false,
                chars: &['A'],
            }),
        ));
    }
    let forward = FontList::index(&paths, &NoProgress).unwrap();
    paths.reverse();
    let backward = FontList::index(&paths, &NoProgress).unwrap();
    assert_eq!(forward, backward);
}

struct Cancelled;
impl Progress for Cancelled {
    fn report(&self, _: u64, _: u64, _: &str) {}
    fn is_cancelled(&self) -> bool {
        true
    }
}

#[test]
fn a_cancel_stops_the_indexing() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(
        dir.path(),
        "a.ttf",
        &font(&Face {
            family: "A",
            weight: 400,
            italic: false,
            mono: false,
            chars: &['A'],
        }),
    );
    assert_eq!(FontList::index(&[p], &Cancelled), Err(FontError::Cancelled));
}

fn family_set(dir: &Path) -> FontList {
    let faces = [
        (
            "sans-400.ttf",
            Face {
                family: "Test Sans",
                weight: 400,
                italic: false,
                mono: false,
                chars: &['A', 'B'],
            },
        ),
        (
            "sans-700.ttf",
            Face {
                family: "Test Sans",
                weight: 700,
                italic: false,
                mono: false,
                chars: &['A', 'B'],
            },
        ),
        (
            "sans-300.ttf",
            Face {
                family: "Test Sans",
                weight: 300,
                italic: false,
                mono: false,
                chars: &['A', 'B'],
            },
        ),
        (
            "sans-400i.ttf",
            Face {
                family: "Test Sans",
                weight: 400,
                italic: true,
                mono: false,
                chars: &['A', 'B'],
            },
        ),
        (
            "serif-400.ttf",
            Face {
                family: "Other Serif",
                weight: 400,
                italic: false,
                mono: false,
                chars: &['A', 'Z'],
            },
        ),
    ];
    let paths: Vec<PathBuf> = faces.iter().map(|(n, f)| write(dir, n, &font(f))).collect();
    FontList::index(&paths, &NoProgress).unwrap()
}

#[test]
fn best_follows_the_weight_search_order_within_a_family() {
    let dir = tempfile::tempdir().unwrap();
    let list = family_set(dir.path());
    let fam = ["Test Sans"];
    let pick = |w: u16, italic: bool| {
        let f = list.best(&query(&fam, w, italic)).unwrap();
        (f.weight, f.italic)
    };
    // Faces: 300, 400, 700 upright; 400 italic.
    assert_eq!(pick(400, false), (400, false));
    assert_eq!(pick(700, false), (700, false));
    assert_eq!(pick(300, false), (300, false));
    // Between 400 and 500 with no 500: 500 and up first (none up to 500), then
    // lighter descending: 400.
    assert_eq!(pick(500, false), (400, false));
    // Above 500: heavier first: 600 is wanted, 700 is the first heavier.
    assert_eq!(pick(600, false), (700, false));
    assert_eq!(pick(900, false), (700, false));
    // Below 400: lighter descending, then heavier: 350 -> 300.
    assert_eq!(pick(350, false), (300, false));
    assert_eq!(pick(100, false), (300, false));
    // Slant is chosen before weight: the italic face wins even when its
    // weight is farther.
    assert_eq!(pick(700, true), (400, true));
}

#[test]
fn family_names_compare_without_case_and_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let list = family_set(dir.path());
    assert_eq!(
        list.best(&query(&["test sans"], 400, false))
            .unwrap()
            .families[0],
        "Test Sans"
    );
    assert_eq!(
        list.best(&query(&["TEST SANS"], 400, false))
            .unwrap()
            .families[0],
        "Test Sans"
    );
    assert_eq!(
        list.best(&query(&["Missing", "Other Serif", "Test Sans"], 400, false))
            .unwrap()
            .families[0],
        "Other Serif"
    );
    assert!(list.best(&query(&["Missing"], 400, false)).is_none());
    assert!(list.best(&query(&[], 400, false)).is_none());
}

#[test]
fn by_family_lists_each_family_closest_style_first_without_repeats() {
    let dir = tempfile::tempdir().unwrap();
    let list = family_set(dir.path());
    let got = names(&list.by_family(&query(
        &["Test Sans", "Other Serif", "test sans"],
        700,
        false,
    )));
    assert_eq!(
        got,
        vec![
            ("Test Sans".to_string(), 700, false),
            // 700 wanted, upright: then heavier (none), lighter descending.
            ("Test Sans".to_string(), 400, false),
            ("Test Sans".to_string(), 300, false),
            ("Test Sans".to_string(), 400, true),
            ("Other Serif".to_string(), 400, false),
        ]
    );
}

#[test]
fn covering_prefers_the_asked_families_then_any_face() {
    let dir = tempfile::tempdir().unwrap();
    let list = family_set(dir.path());
    let fam = ["Test Sans"];
    let q = query(&fam, 400, false);
    // 'A' is in Test Sans.
    assert_eq!(list.covering('A', &q).unwrap().families[0], "Test Sans");
    // 'Z' is only in Other Serif: found by falling back to the whole list.
    assert_eq!(list.covering('Z', &q).unwrap().families[0], "Other Serif");
    // Nothing has 'Q'.
    assert!(list.covering('Q', &q).is_none());
    // Asking for Other Serif first gives it for 'A' too, which it also has.
    let serif = ["Other Serif", "Test Sans"];
    assert_eq!(
        list.covering('A', &query(&serif, 400, false))
            .unwrap()
            .families[0],
        "Other Serif"
    );
    assert_eq!(
        list.covering('B', &query(&serif, 400, false))
            .unwrap()
            .families[0],
        "Test Sans"
    );
}

#[test]
fn a_type_1_file_is_listed_by_name_with_no_coverage() {
    let dir = tempfile::tempdir().unwrap();
    let spec = Type1Spec::new(vec![(
        ".notdef",
        Cs::new().with(&[0, 250], "hsbw").op("endchar"),
    )]);
    let p = write(dir.path(), "t1.pfa", &spec.hex());
    let list = FontList::index(&[p], &NoProgress).unwrap();
    let [face] = list.faces() else {
        panic!("one face expected")
    };
    assert_eq!(face.families, vec!["Test".to_string()]);
    assert_eq!(face.postscript_name.as_deref(), Some("TestFont"));
    assert!(face.italic, "ItalicAngle -12");
    assert!(face.monospaced);
    assert!(face.coverage.is_none());
    assert!(!face.covers('A'));
    // Found by family, never by coverage.
    assert!(list.best(&query(&["Test"], 400, false)).is_some());
    assert!(list.covering('A', &query(&["Test"], 400, false)).is_none());
}
