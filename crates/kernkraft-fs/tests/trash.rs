// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use kernkraft_fs::{
    choose_target, decode_trash_path, encode_trash_path, parse_trashinfo, FsError, Trash,
    TrashTarget,
};

struct Fixture {
    _root: tempfile::TempDir,
    trash_dir: PathBuf,
    src: PathBuf,
    trash: Trash,
}

fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let trash_dir = root.path().join("trash");
    let src = root.path().canonicalize().unwrap().join("src");
    fs::create_dir(&src).unwrap();
    let trash = Trash::with_home_trash(trash_dir.clone());
    Fixture {
        _root: root,
        trash_dir,
        src,
        trash,
    }
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn put_moves_file_and_writes_info() {
    let f = fixture();
    let file = f.src.join("a b.txt");
    fs::write(&file, "data").unwrap();
    let item = f.trash.put(&file).unwrap();

    assert!(!file.exists());
    assert_eq!(item.name, "a b.txt");
    assert_eq!(item.original, file);
    assert!(!item.is_dir);
    assert_eq!(
        fs::read_to_string(f.trash_dir.join("files/a b.txt")).unwrap(),
        "data"
    );
    let info = fs::read_to_string(f.trash_dir.join("info/a b.txt.trashinfo")).unwrap();
    let (path, date) = parse_trashinfo(&info).unwrap();
    assert_eq!(path, encode_trash_path(&file));
    assert!(path.contains("a%20b.txt"));
    assert_eq!(date, item.deleted);
    // YYYY-MM-DDThh:mm:ss
    assert_eq!(date.len(), 19);
    assert_eq!(&date[10..11], "T");
}

#[test]
fn trash_dirs_are_private() {
    let f = fixture();
    let file = f.src.join("a.txt");
    fs::write(&file, "").unwrap();
    f.trash.put(&file).unwrap();
    for sub in ["files", "info"] {
        let mode = fs::metadata(f.trash_dir.join(sub))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700, "{sub}");
    }
}

#[test]
fn put_twice_same_name_disambiguates() {
    let f = fixture();
    let file = f.src.join("a.txt");
    fs::write(&file, "one").unwrap();
    let first = f.trash.put(&file).unwrap();
    fs::write(&file, "two").unwrap();
    let second = f.trash.put(&file).unwrap();
    assert_eq!(first.name, "a.txt");
    assert_eq!(second.name, "a (2).txt");
    assert_eq!(
        fs::read_to_string(f.trash_dir.join("files/a (2).txt")).unwrap(),
        "two"
    );
}

#[test]
fn put_avoids_orphaned_file_name() {
    let f = fixture();
    fs::create_dir_all(f.trash_dir.join("files")).unwrap();
    fs::create_dir_all(f.trash_dir.join("info")).unwrap();
    fs::write(f.trash_dir.join("files/a.txt"), "orphan").unwrap();
    let file = f.src.join("a.txt");
    fs::write(&file, "new").unwrap();
    let item = f.trash.put(&file).unwrap();
    assert_eq!(item.name, "a (2).txt");
    assert_eq!(
        fs::read_to_string(f.trash_dir.join("files/a.txt")).unwrap(),
        "orphan"
    );
}

#[test]
fn put_directory_marks_is_dir() {
    let f = fixture();
    let dir = f.src.join("d.v1");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("x"), "x").unwrap();
    let item = f.trash.put(&dir).unwrap();
    assert!(item.is_dir);
    assert_eq!(
        fs::read_to_string(f.trash_dir.join("files/d.v1/x")).unwrap(),
        "x"
    );
}

#[test]
fn put_symlink_trashes_the_link_not_its_target() {
    let f = fixture();
    let target = f.src.join("target.txt");
    fs::write(&target, "keep").unwrap();
    let link = f.src.join("link");
    symlink(&target, &link).unwrap();
    let item = f.trash.put(&link).unwrap();
    assert!(!item.is_dir);
    assert_eq!(fs::read_to_string(&target).unwrap(), "keep");
    assert!(fs::symlink_metadata(f.trash_dir.join("files/link"))
        .unwrap()
        .file_type()
        .is_symlink());
}

#[test]
fn put_missing_path_is_not_found() {
    let f = fixture();
    let err = f.trash.put(&f.src.join("nope")).unwrap_err();
    assert!(matches!(err, FsError::NotFound { .. }), "{err:?}");
}

#[test]
fn failed_move_removes_its_info_file() {
    let f = fixture();
    // The trash cannot be moved into itself; the kernel refuses the rename.
    fs::create_dir_all(&f.trash_dir).unwrap();
    let err = f.trash.put(&f.trash_dir).unwrap_err();
    assert!(matches!(err, FsError::Io { .. }), "{err:?}");
    assert!(names(&f.trash_dir.join("info")).is_empty());
}

#[test]
fn list_returns_what_put_put() {
    let f = fixture();
    let file = f.src.join("a.txt");
    fs::write(&file, "").unwrap();
    let item = f.trash.put(&file).unwrap();
    let listing = f.trash.list().unwrap();
    assert!(listing.unreadable.is_empty(), "{:?}", listing.unreadable);
    let mine: Vec<_> = listing
        .items
        .iter()
        .filter(|i| i.trash_dir == f.trash_dir)
        .collect();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0], &item);
}

#[test]
fn list_reports_malformed_info() {
    let f = fixture();
    fs::create_dir_all(f.trash_dir.join("files")).unwrap();
    fs::create_dir_all(f.trash_dir.join("info")).unwrap();
    fs::write(f.trash_dir.join("files/bad"), "").unwrap();
    fs::write(f.trash_dir.join("info/bad.trashinfo"), "garbage").unwrap();
    let listing = f.trash.list().unwrap();
    assert!(listing.items.iter().all(|i| i.trash_dir != f.trash_dir));
    assert_eq!(listing.unreadable.len(), 1);
    assert!(matches!(
        listing.unreadable[0].1,
        FsError::BadTrashInfo { .. }
    ));
}

#[test]
fn list_of_missing_home_trash_is_empty() {
    let f = fixture();
    let listing = f.trash.list().unwrap();
    assert!(listing.items.iter().all(|i| i.trash_dir != f.trash_dir));
    assert!(listing.unreadable.is_empty());
}

#[test]
fn restore_puts_file_back() {
    let f = fixture();
    let file = f.src.join("sub/a.txt");
    fs::create_dir(f.src.join("sub")).unwrap();
    fs::write(&file, "data").unwrap();
    let item = f.trash.put(&file).unwrap();
    fs::remove_dir(f.src.join("sub")).unwrap();

    let restored = f.trash.restore(&item).unwrap();
    assert_eq!(restored, file);
    assert_eq!(fs::read_to_string(&file).unwrap(), "data");
    assert!(names(&f.trash_dir.join("files")).is_empty());
    assert!(names(&f.trash_dir.join("info")).is_empty());
}

#[test]
fn restore_onto_existing_is_refused() {
    let f = fixture();
    let file = f.src.join("a.txt");
    fs::write(&file, "trashed").unwrap();
    let item = f.trash.put(&file).unwrap();
    fs::write(&file, "newer").unwrap();

    let err = f.trash.restore(&item).unwrap_err();
    assert!(matches!(err, FsError::Exists { .. }), "{err:?}");
    assert_eq!(fs::read_to_string(&file).unwrap(), "newer");
    assert_eq!(names(&f.trash_dir.join("files")), ["a.txt"]);
    assert_eq!(names(&f.trash_dir.join("info")), ["a.txt.trashinfo"]);
}

#[test]
fn purge_removes_both_entries() {
    let f = fixture();
    let dir = f.src.join("d");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("x"), "").unwrap();
    let item = f.trash.put(&dir).unwrap();
    f.trash.purge(&item).unwrap();
    assert!(names(&f.trash_dir.join("files")).is_empty());
    assert!(names(&f.trash_dir.join("info")).is_empty());
}

#[test]
fn purge_does_not_follow_symlinks_out_of_the_trash() {
    let f = fixture();
    let outside = f.src.join("outside.txt");
    fs::write(&outside, "keep").unwrap();
    let dir = f.src.join("d");
    fs::create_dir(&dir).unwrap();
    symlink(&outside, dir.join("link")).unwrap();
    let item = f.trash.put(&dir).unwrap();
    f.trash.purge(&item).unwrap();
    assert_eq!(fs::read_to_string(&outside).unwrap(), "keep");
}

#[test]
fn encode_escapes_space_and_percent() {
    assert_eq!(encode_trash_path(Path::new("/a b/%c")), "/a%20b/%25c");
    assert_eq!(
        encode_trash_path(Path::new("/x-y_z.~/é")),
        "/x-y_z.~/%C3%A9"
    );
}

#[test]
fn decode_reverses_encode_for_non_utf8() {
    use std::os::unix::ffi::OsStrExt;
    let raw = Path::new(std::ffi::OsStr::from_bytes(b"/a/\xff b"));
    assert_eq!(decode_trash_path(&encode_trash_path(raw)), raw);
}

#[test]
fn parse_reads_keys_in_trash_info_group() {
    let parsed = parse_trashinfo(
        "[Trash Info]\nPath=/a%20b\nDeletionDate=2026-10-18T12:00:00\n\n[Other]\nPath=/x\n",
    );
    assert_eq!(
        parsed,
        Some(("/a%20b".to_string(), "2026-10-18T12:00:00".to_string()))
    );
}

#[test]
fn parse_rejects_missing_header() {
    assert_eq!(
        parse_trashinfo("Path=/a\nDeletionDate=2026-10-18T12:00:00\n"),
        None
    );
}

#[test]
fn parse_rejects_missing_key() {
    assert_eq!(parse_trashinfo("[Trash Info]\nPath=/a\n"), None);
}

#[test]
fn choose_same_device_is_home() {
    let t = choose_target(5, 5, Path::new("/mnt"), 1000, |_| true);
    assert_eq!(t, TrashTarget::Home);
}

#[test]
fn choose_other_device_prefers_sticky_shared() {
    let t = choose_target(6, 5, Path::new("/mnt"), 1000, |_| true);
    assert_eq!(t, TrashTarget::TopDir(PathBuf::from("/mnt/.Trash/1000")));
}

#[test]
fn choose_other_device_falls_back_to_user_dir() {
    let t = choose_target(6, 5, Path::new("/mnt"), 1000, |_| false);
    assert_eq!(
        t,
        TrashTarget::TopDirUser(PathBuf::from("/mnt/.Trash-1000"))
    );
}

#[test]
fn list_includes_topdir_user_trash() {
    let root = tempfile::tempdir().unwrap();
    let topdir = root.path().join("mnt");
    // A directory this process created is owned by its uid.
    let uid = fs::metadata(root.path()).unwrap().uid();
    let trash_dir = topdir.join(format!(".Trash-{uid}"));
    fs::create_dir_all(trash_dir.join("files")).unwrap();
    fs::create_dir_all(trash_dir.join("info")).unwrap();
    fs::write(trash_dir.join("files/r.txt"), "").unwrap();
    fs::write(
        trash_dir.join("info/r.txt.trashinfo"),
        "[Trash Info]\nPath=docs/r.txt\nDeletionDate=2026-10-18T12:00:00\n",
    )
    .unwrap();
    let trash = Trash::with_dirs(root.path().join("home-trash"), vec![topdir.clone()]);
    let listing = trash.list().unwrap();
    assert!(listing.unreadable.is_empty(), "{:?}", listing.unreadable);
    assert_eq!(listing.items.len(), 1);
    assert_eq!(listing.items[0].original, topdir.join("docs/r.txt"));
    assert_eq!(listing.items[0].trash_dir, trash_dir);
}

#[test]
fn list_reports_non_utf8_trash_name() {
    use std::os::unix::ffi::OsStrExt;
    let f = fixture();
    fs::create_dir_all(f.trash_dir.join("files")).unwrap();
    fs::create_dir_all(f.trash_dir.join("info")).unwrap();
    let info = f
        .trash_dir
        .join("info")
        .join(std::ffi::OsStr::from_bytes(b"\xff.trashinfo"));
    fs::write(
        info,
        "[Trash Info]\nPath=/x\nDeletionDate=2026-10-18T12:00:00\n",
    )
    .unwrap();
    let listing = f.trash.list().unwrap();
    assert_eq!(listing.unreadable.len(), 1);
    assert!(matches!(
        listing.unreadable[0].1,
        FsError::InvalidName { .. }
    ));
}
