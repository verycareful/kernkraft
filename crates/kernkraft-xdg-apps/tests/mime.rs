// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Tests for `MimeDb` against the hand-written fixture database in
//! `tests/data/mime/` (Shared MIME-info Database 0.21, sections 2.4, 2.5 and
//! 2.11). Each test names the fixture lines it relies on.

use std::fs;
use std::path::{Path, PathBuf};

use kernkraft_xdg_apps::{AppsError, MimeDb, SNIFF_LEN};

fn fixture_dir() -> PathBuf {
    // The fixture's `mime/` folder is `tests/data/mime`, so its data dir is
    // `tests/data`.
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data")
}

fn db() -> MimeDb {
    MimeDb::load(&[fixture_dir()]).unwrap()
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, bytes).unwrap();
    path
}

// Relies on globs2: `10:application/x-low:*.wt` and
// `80:application/x-high:*.wt`.
#[test]
fn glob_weight_wins() {
    assert_eq!(db().by_name("a.wt"), Some("application/x-high"));
}

// Relies on globs2: `50:application/x-gzip-like:*.gz` and
// `50:application/x-tar-gzip-like:*.tar.gz`, equal weight.
#[test]
fn longest_pattern_wins_on_tie() {
    let db = db();
    assert_eq!(db.by_name("a.tar.gz"), Some("application/x-tar-gzip-like"));
    assert_eq!(db.by_name("a.gz"), Some("application/x-gzip-like"));
}

// Relies on globs2: `50:application/x-upper-only:*.UP:cs` (case-sensitive)
// and `50:application/x-any-case:*.Any` (case-insensitive).
#[test]
fn case_sensitive_glob_respected() {
    let db = db();
    assert_eq!(db.by_name("a.UP"), Some("application/x-upper-only"));
    assert_eq!(db.by_name("a.up"), None);
    assert_eq!(db.by_name("a.ANY"), Some("application/x-any-case"));
    assert_eq!(db.by_name("a.any"), Some("application/x-any-case"));
}

// Relies on globs2: `50:text/x-makefile:Makefile` against the heavier
// `90:application/x-make-glob:Make*`.
#[test]
fn literal_beats_heavier_glob() {
    let db = db();
    assert_eq!(db.by_name("Makefile"), Some("text/x-makefile"));
    assert_eq!(db.by_name("Makeshift"), Some("application/x-make-glob"));
}

// Relies on globs2: `set[0-9].bin` and `q?.dat`.
#[test]
fn bracket_and_question_globs() {
    let db = db();
    assert_eq!(db.by_name("set7.bin"), Some("application/x-set"));
    assert_eq!(db.by_name("setx.bin"), None);
    assert_eq!(db.by_name("qz.dat"), Some("application/x-question"));
    assert_eq!(db.by_name("q.dat"), None);
}

// Relies on aliases: `application/x-javascript application/javascript`,
// subclasses: `application/javascript text/plain`.
#[test]
fn alias_resolves() {
    let db = db();
    assert!(db.is_a("application/x-javascript", "application/javascript"));
    assert!(db.is_a("application/javascript", "application/x-javascript"));
    assert!(db.is_text("application/x-javascript"));
}

// Relies on subclasses: `application/json application/javascript` and
// `application/javascript text/plain`.
#[test]
fn json_is_text_via_subclass() {
    let db = db();
    assert!(db.is_a("application/json", "text/plain"));
    assert!(db.is_text("application/json"));
    assert!(!db.is_a("text/plain", "application/json"));
    assert!(!db.is_text("application/octet-stream"));
}

// Section 2.5: every text/* type is a text/plain subclass, with no line
// for it in the fixture (`text/markdown` has none).
#[test]
fn text_prefix_is_text() {
    let db = db();
    assert!(db.is_text("text/markdown"));
    assert!(db.is_a("text/x-unlisted", "text/plain"));
}

// Relies on subclasses: the `application/x-loop-a` and
// `application/x-loop-b` lines, which point at each other.
#[test]
fn subclass_cycle_terminates() {
    let db = db();
    assert!(!db.is_a("application/x-loop-a", "text/plain"));
    assert!(db.is_a("application/x-loop-a", "application/x-loop-b"));
}

// Relies on no pattern in globs2 matching `*.zzz`.
#[test]
fn unknown_utf8_sniffs_text() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "notes.zzz", "héllo\nwörld\n".as_bytes());
    assert_eq!(db().guess(&path), "text/plain");
}

#[test]
fn nul_byte_sniffs_binary() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "blob.zzz", b"abc\0def");
    assert_eq!(db().guess(&path), "application/octet-stream");
}

#[test]
fn invalid_utf8_sniffs_binary() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "blob.zzz", &[b'a', 0xFF, 0xFE, b'b']);
    assert_eq!(db().guess(&path), "application/octet-stream");
}

#[test]
fn cut_multibyte_at_window_end_is_text() {
    // The 2-byte char U+00E9 starts at the last byte of the window, so the
    // window ends mid-sequence although the file is valid UTF-8.
    let mut bytes = vec![b'a'; SNIFF_LEN - 1];
    bytes.extend_from_slice("é".as_bytes());
    assert_eq!(bytes.len(), SNIFF_LEN + 1);
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "long.zzz", &bytes);
    assert_eq!(db().guess(&path), "text/plain");
}

#[test]
fn truncated_multibyte_in_short_file_is_binary() {
    // The file itself ends mid-sequence (first byte of U+00E9 only).
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "short.zzz", &[b'a', 0xC3]);
    assert_eq!(db().guess(&path), "application/octet-stream");
}

#[test]
fn nul_after_window_is_not_seen() {
    let mut bytes = vec![b'a'; SNIFF_LEN];
    bytes.push(0);
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "late.zzz", &bytes);
    assert_eq!(db().guess(&path), "text/plain");
}

#[test]
fn directory_is_inode_directory() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("looks-like.md");
    fs::create_dir(&sub).unwrap();
    assert_eq!(db().guess(&sub), "inode/directory");
}

// Relies on globs2: `50:text/markdown:*.md`; the link itself has no
// extension, the target does.
#[test]
fn symlink_guessed_by_target() {
    let dir = tempfile::tempdir().unwrap();
    let target = write(dir.path(), "readme.md", b"\0\0\0");
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert_eq!(db().guess(&link), "text/markdown");
    let dir_link = dir.path().join("dirlink");
    std::os::unix::fs::symlink(dir.path(), &dir_link).unwrap();
    assert_eq!(db().guess(&dir_link), "inode/directory");
}

// A link whose target cannot be resolved is not a file anyone can open,
// whatever its own name says (`gone.md` matches `50:text/markdown:*.md`).
#[test]
fn dangling_symlink_is_inode_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let link = dir.path().join("gone.md");
    std::os::unix::fs::symlink(dir.path().join("missing"), &link).unwrap();
    assert_eq!(db().guess(&link), "inode/symlink");
    let a = dir.path().join("loop-a");
    let b = dir.path().join("loop-b");
    std::os::unix::fs::symlink(&b, &a).unwrap();
    std::os::unix::fs::symlink(&a, &b).unwrap();
    assert_eq!(db().guess(&a), "inode/symlink");
}

// Relies on no pattern in globs2 matching `*.zzz`: an empty file is text.
#[test]
fn empty_file_is_text_plain() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "empty.zzz", b"");
    assert_eq!(db().guess(&path), "text/plain");
}

// Relies on globs2: `50:text/markdown:*.md`. A path that does not exist is
// typed by its name alone; an unknown name gets octet-stream.
#[test]
fn missing_path_is_typed_by_name() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(db().guess(&dir.path().join("new.md")), "text/markdown");
}

#[test]
fn missing_file_is_octet_stream() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        db().guess(&dir.path().join("nope.zzz")),
        "application/octet-stream"
    );
}

#[test]
fn fifo_is_not_opened() {
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("pipe");
    make_fifo(&fifo);
    assert_eq!(db().guess(&fifo), "inode/fifo");
}

fn make_fifo(path: &Path) {
    let status = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn missing_mime_dir_is_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let db = MimeDb::load(&[dir.path().to_path_buf(), dir.path().join("absent")]).unwrap();
    assert_eq!(db.by_name("a.txt"), None);
}

// Earlier data dir wins an equal-weight, equal-length tie: both dirs say
// `*.same`, with different types.
#[test]
fn earlier_dir_wins_tie() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    for (dir, mime) in [(&first, "a/first"), (&second, "a/second")] {
        fs::create_dir(dir.path().join("mime")).unwrap();
        fs::write(
            dir.path().join("mime/globs2"),
            format!("50:{mime}:*.same\n"),
        )
        .unwrap();
    }
    let db = MimeDb::load(&[first.path().to_path_buf(), second.path().to_path_buf()]).unwrap();
    assert_eq!(db.by_name("x.same"), Some("a/first"));
}

fn malformed(files: &[(&str, &str)]) -> AppsError {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("mime")).unwrap();
    for (name, body) in files {
        fs::write(dir.path().join("mime").join(name), body).unwrap();
    }
    MimeDb::load(&[dir.path().to_path_buf()]).unwrap_err()
}

#[test]
fn malformed_lines_fail_the_load_with_line_number() {
    for (file, body, line) in [
        ("globs2", "50:text/plain:*.txt\nnot a glob line\n", 2),
        ("globs2", "x:text/plain:*.txt\n", 1),
        ("globs2", "50:notamime:*.txt\n", 1),
        ("globs2", "50:text/plain:\n", 1),
        ("subclasses", "# c\na/b\n", 2),
        ("aliases", "a/b c/d e/f\n", 1),
    ] {
        match malformed(&[(file, body)]) {
            AppsError::MalformedMimeDb { path, line: got } => {
                assert!(path.ends_with(file), "{path:?}");
                assert_eq!(got, line, "{file}: {body:?}");
            }
            other => panic!("expected MalformedMimeDb, got {other:?}"),
        }
    }
}

// Reads the machine's real database; skipped (with a note) where it is
// absent.
#[test]
fn system_db_knows_markdown() {
    if !Path::new("/usr/share/mime/globs2").exists() {
        eprintln!("skipping system_db_knows_markdown: /usr/share/mime/globs2 absent");
        return;
    }
    let db = MimeDb::load(&[PathBuf::from("/usr/share")]).unwrap();
    assert_eq!(db.by_name("a.md"), Some("text/markdown"));
    assert!(db.is_text("text/markdown"));
}

// The higher-priority dir `tests/data_user` has
// `50:application/x-high:__NOGLOBS__`; the lower one, `tests/data`, has
// `80:application/x-high:*.wt` and `10:application/x-low:*.wt`.
#[test]
fn noglobs_in_user_dir_drops_system_globs() {
    let user = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data_user");
    let with_user_first = MimeDb::load(&[user.clone(), fixture_dir()]).unwrap();
    assert_eq!(with_user_first.by_name("a.wt"), Some("application/x-low"));
    // In the other order the marker belongs to a lower-priority dir and
    // drops nothing above it.
    let with_user_last = MimeDb::load(&[fixture_dir(), user]).unwrap();
    assert_eq!(with_user_last.by_name("a.wt"), Some("application/x-high"));
}

/// One globs2 line for the reference scan.
struct RefPattern {
    weight: u32,
    mime: String,
    pattern: Vec<char>,
    cs: bool,
}

fn ref_match(p: &[char], t: &[char]) -> bool {
    match p.first() {
        None => t.is_empty(),
        Some('*') => (0..=t.len()).any(|i| ref_match(&p[1..], &t[i..])),
        Some('?') => !t.is_empty() && ref_match(&p[1..], &t[1..]),
        Some('[') => match p.iter().position(|&c| c == ']') {
            Some(close) if close > 1 && !t.is_empty() => {
                let set = &p[1..close];
                let mut hit = false;
                let mut i = 0;
                while i < set.len() {
                    if i + 2 < set.len() && set[i + 1] == '-' {
                        hit |= set[i] <= t[0] && t[0] <= set[i + 2];
                        i += 3;
                    } else {
                        hit |= set[i] == t[0];
                        i += 1;
                    }
                }
                hit && ref_match(&p[close + 1..], &t[1..])
            }
            _ => false,
        },
        Some(&c) => !t.is_empty() && c == t[0] && ref_match(&p[1..], &t[1..]),
    }
}

/// The straightforward algorithm of section 2.11 over every line.
fn reference_by_name<'a>(all: &'a [RefPattern], name: &str) -> Option<&'a str> {
    let lower = name.to_lowercase();
    let pick = |literal: bool| {
        all.iter()
            .filter(|p| p.pattern.iter().any(|c| matches!(c, '*' | '?' | '[')) != literal)
            .filter(|p| {
                let subject: Vec<char> = if p.cs { name } else { lower.as_str() }.chars().collect();
                if literal {
                    subject == p.pattern
                } else {
                    ref_match(&p.pattern, &subject)
                }
            })
            // Complete ties go to the line listed first, so `max_by_key`
            // (which keeps the last maximum) is not used.
            .reduce(|best, p| {
                if (p.weight, p.pattern.len(), p.cs) > (best.weight, best.pattern.len(), best.cs) {
                    p
                } else {
                    best
                }
            })
            .map(|p| p.mime.as_str())
    };
    pick(true).or_else(|| pick(false))
}

fn read_system_globs() -> Option<Vec<RefPattern>> {
    let text = fs::read_to_string("/usr/share/mime/globs2").ok()?;
    let mut out = Vec::new();
    for line in text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let mut f = line.splitn(4, ':');
        let weight = f.next()?.parse().ok()?;
        let mime = f.next()?.to_string();
        let pattern = f.next()?;
        let cs = f.next().is_some_and(|fl| fl.split(',').any(|x| x == "cs"));
        let pattern = if cs {
            pattern.to_string()
        } else {
            pattern.to_lowercase()
        };
        out.push(RefPattern {
            weight,
            mime,
            pattern: pattern.chars().collect(),
            cs,
        });
    }
    Some(out)
}

// Compares the index with the reference scan on every literal and `*.ext`
// suffix of the system globs2, in three letter cases, and on tricky names.
// Equal-weight, equal-length ties between directories cannot arise with a
// single data dir, so the reference needs no directory rank.
#[test]
fn by_name_agrees_with_linear_scan() {
    let Some(all) = read_system_globs() else {
        eprintln!("skipping by_name_agrees_with_linear_scan: /usr/share/mime/globs2 absent");
        return;
    };
    let db = MimeDb::load(&[PathBuf::from("/usr/share")]).unwrap();
    let mut names: Vec<String> = [
        "a.tar.gz",
        "Makefile",
        "makefile",
        "README",
        "readme",
        "x.C",
        "x.c",
        ".bashrc",
        "noext",
        "",
        ".",
        "a.",
        "..",
        "a.TAR.GZ",
        "libx.so.1",
        "libx.so.12",
        "libx.so",
        "a.b.c.d.md",
        ".md",
        "caf\u{e9}.md",
        "A.MD",
        "core",
        "Core",
        "perf.data",
        "Perf.Data",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for p in &all {
        let text: String = p.pattern.iter().collect();
        let base = match text.strip_prefix('*') {
            Some(rest) if !rest.contains(['*', '?', '[']) => format!("file{rest}"),
            _ => text.clone(),
        };
        names.push(base.to_uppercase());
        names.push(base.to_lowercase());
        names.push(base);
    }
    for name in &names {
        assert_eq!(
            db.by_name(name),
            reference_by_name(&all, name),
            "name {name:?}"
        );
    }
}

// Budget: 200 ms for 100 000 names, the plan's folder-view budget for
// sorting 100 000 entries, so typing every entry must not cost more than
// sorting them. This runs in `cargo test`'s default (debug) profile, which is
// slower than release; the budget is set for that profile.
#[test]
fn by_name_100k_under_budget() {
    if !Path::new("/usr/share/mime/globs2").exists() {
        eprintln!("skipping by_name_100k_under_budget: /usr/share/mime/globs2 absent");
        return;
    }
    let db = MimeDb::load(&[PathBuf::from("/usr/share")]).unwrap();
    let exts = [
        "txt",
        "md",
        "tar.gz",
        "png",
        "JPG",
        "rs",
        "so.3",
        "unknownext",
        "c",
        "C",
        "",
    ];
    let names: Vec<String> = (0..100_000)
        .map(|i| match exts[i % exts.len()] {
            "" => format!("file-{i}"),
            ext => format!("file-{i}.{ext}"),
        })
        .collect();
    let start = std::time::Instant::now();
    let mut typed = 0usize;
    for name in &names {
        if db.by_name(name).is_some() {
            typed += 1;
        }
    }
    let elapsed = start.elapsed();
    eprintln!("100 000 by_name calls: {elapsed:?}");
    assert!(typed > 50_000, "typed {typed}");
    assert!(
        elapsed < std::time::Duration::from_millis(200),
        "100 000 lookups took {elapsed:?}"
    );
}
