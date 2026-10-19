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

// Relies on globs2: `50:text/markdown:*.md`; a dangling link falls back to
// its own name.
#[test]
fn dangling_symlink_uses_own_name() {
    let dir = tempfile::tempdir().unwrap();
    let link = dir.path().join("gone.md");
    std::os::unix::fs::symlink(dir.path().join("missing"), &link).unwrap();
    assert_eq!(db().guess(&link), "text/markdown");
    let bare = dir.path().join("gone");
    std::os::unix::fs::symlink(dir.path().join("missing"), &bare).unwrap();
    assert_eq!(db().guess(&bare), "application/octet-stream");
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
