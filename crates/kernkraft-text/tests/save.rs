// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;

use kernkraft_fs::FsError;
use kernkraft_text::{open, save, LineEnding, ReadOnly, TextError, EDITABLE_LIMIT, OPEN_LIMIT};

#[test]
fn save_preserves_crlf_and_bom() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.txt");
    fs::write(&path, b"\xEF\xBB\xBFhello world\r\nsecond line\r\n").unwrap();

    let file = open(&path).unwrap();
    assert!(file.decoded.read_only.is_none());
    let edited = file.decoded.text.replace("world", "there");
    save(
        &path,
        &edited,
        file.decoded.ending,
        file.decoded.bom,
        Some(&file.stamp),
    )
    .unwrap();

    assert_eq!(
        fs::read(&path).unwrap(),
        b"\xEF\xBB\xBFhello there\r\nsecond line\r\n"
    );
}

#[test]
fn save_returns_the_stamp_of_the_new_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.txt");
    fs::write(&path, "a\n").unwrap();
    let file = open(&path).unwrap();
    let stamp = save(&path, "b\n", LineEnding::Lf, false, Some(&file.stamp)).unwrap();
    assert_eq!(stamp, open(&path).unwrap().stamp);
}

#[test]
fn save_detects_external_change() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.txt");
    fs::write(&path, "one\n").unwrap();
    let file = open(&path).unwrap();

    fs::write(&path, "changed elsewhere, longer\n").unwrap();

    let err = save(&path, "mine\n", LineEnding::Lf, false, Some(&file.stamp)).unwrap_err();
    assert!(
        matches!(err, TextError::Fs(FsError::ExternalModification { .. })),
        "{err:?}"
    );
    assert_eq!(fs::read(&path).unwrap(), b"changed elsewhere, longer\n");
}

#[test]
fn open_over_editable_limit_is_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.log");
    let len = EDITABLE_LIMIT + 1;
    fs::write(&path, vec![b'x'; usize::try_from(len).unwrap()]).unwrap();
    let file = open(&path).unwrap();
    assert_eq!(
        file.decoded.read_only,
        Some(ReadOnly::TooLarge {
            len,
            limit: EDITABLE_LIMIT
        })
    );
    assert_eq!(file.decoded.text.len() as u64, len);
}

#[test]
fn open_at_editable_limit_is_editable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("edge.txt");
    fs::write(&path, vec![b'x'; usize::try_from(EDITABLE_LIMIT).unwrap()]).unwrap();
    assert_eq!(open(&path).unwrap().decoded.read_only, None);
}

#[test]
fn open_over_open_limit_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("huge.bin");
    File::create(&path)
        .unwrap()
        .set_len(OPEN_LIMIT + 1)
        .unwrap();
    let err = open(&path).unwrap_err();
    assert!(
        matches!(
            err,
            TextError::TooLargeToOpen { len, limit } if len == OPEN_LIMIT + 1 && limit == OPEN_LIMIT
        ),
        "{err:?}"
    );
}

#[test]
fn open_missing_file_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let err = open(&dir.path().join("none.txt")).unwrap_err();
    assert!(matches!(err, TextError::Fs(_)), "{err:?}");
}

#[test]
fn too_large_outranks_decode_reasons() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.txt");
    let mut bytes = vec![b'x'; usize::try_from(EDITABLE_LIMIT).unwrap()];
    bytes.push(0xFF);
    fs::write(&path, bytes).unwrap();
    let file = open(&path).unwrap();
    assert!(matches!(
        file.decoded.read_only,
        Some(ReadOnly::TooLarge { .. })
    ));
}

#[test]
fn no_write_permission_read_only() {
    if rustix::process::geteuid().is_root() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ro.txt");
    fs::write(&path, "text\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
    let file = open(&path).unwrap();
    assert_eq!(file.decoded.read_only, Some(ReadOnly::NoWritePermission));
    assert_eq!(file.decoded.text, "text\n");
}

#[test]
fn no_write_permission_outranks_decode_reasons() {
    if rustix::process::geteuid().is_root() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ro.txt");
    fs::write(&path, b"a\r\nb\nc").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
    assert_eq!(
        open(&path).unwrap().decoded.read_only,
        Some(ReadOnly::NoWritePermission)
    );
}
