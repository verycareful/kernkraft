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
    assert!(
        matches!(err, TextError::Fs(FsError::NotFound { .. })),
        "{err:?}"
    );
}

#[test]
fn open_fifo_refused_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pipe");
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &path,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::from_raw_mode(0o600),
        0,
    )
    .unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let thread_path = path.clone();
    std::thread::spawn(move || {
        let _ = tx.send(open(&thread_path));
    });
    let result = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("open blocked on a FIFO");
    match result.unwrap_err() {
        TextError::Fs(FsError::Io { path: p, source }) => {
            assert_eq!(p, path);
            assert_eq!(source.kind(), std::io::ErrorKind::InvalidInput);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn open_directory_refused() {
    let dir = tempfile::tempdir().unwrap();
    let err = open(dir.path()).unwrap_err();
    assert!(matches!(err, TextError::Fs(FsError::Io { .. })), "{err:?}");
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

/// Every content reason a file opens read-only for: saving the lossy copy
/// over it would replace the original bytes.
fn lossy_files() -> Vec<(&'static str, &'static [u8])> {
    vec![
        // 0xE9 alone is not UTF-8 (RFC 3629 section 4).
        ("NotUtf8", b"caf\xE9\n"),
        // UTF-16 LE byte order mark (Unicode Standard 23.8).
        ("Utf16", b"\xFF\xFEh\0i\0"),
        ("MixedLineEndings", b"a\r\nb\n"),
    ]
}

#[test]
fn save_with_stamp_refuses_lossy_content() {
    for (kind, bytes) in lossy_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        fs::write(&path, bytes).unwrap();
        let file = open(&path).unwrap();
        assert!(file.decoded.read_only.is_some(), "{kind}");
        // A UI that offered Save anyway: the lossy text goes nowhere.
        let err = save(
            &path,
            &file.decoded.text,
            file.decoded.ending,
            file.decoded.bom,
            Some(&file.stamp),
        )
        .unwrap_err();
        match &err {
            TextError::ReadOnly { path: p, reason } => {
                assert_eq!(p, &path, "{kind}");
                assert_eq!(format!("{reason:?}").split([' ', '{']).next(), Some(kind));
            }
            other => panic!("{kind}: {other:?}"),
        }
        assert_eq!(fs::read(&path).unwrap(), bytes, "{kind}");
    }
}

#[test]
fn overwrite_without_stamp_refuses_lossy_content() {
    for (kind, bytes) in lossy_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        fs::write(&path, bytes).unwrap();
        let err = save(&path, "new\n", LineEnding::Lf, false, None).unwrap_err();
        assert!(matches!(err, TextError::ReadOnly { .. }), "{kind}: {err:?}");
        assert_eq!(fs::read(&path).unwrap(), bytes, "{kind}");
    }
}

#[test]
fn save_as_to_a_new_path_and_overwrite_of_clean_text_write() {
    let dir = tempfile::tempdir().unwrap();
    let new = dir.path().join("new.txt");
    save(&new, "n\n", LineEnding::CrLf, false, None).unwrap();
    assert_eq!(fs::read(&new).unwrap(), b"n\r\n");
    // An existing file that decodes cleanly may be overwritten unstamped.
    save(&new, "m\n", LineEnding::Lf, false, None).unwrap();
    assert_eq!(fs::read(&new).unwrap(), b"m\n");
}

#[test]
fn save_with_stamp_of_deleted_file_is_external_modification() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.txt");
    fs::write(&path, "a").unwrap();
    let file = open(&path).unwrap();
    fs::remove_file(&path).unwrap();
    let err = save(&path, "b", LineEnding::Lf, false, Some(&file.stamp)).unwrap_err();
    assert!(
        matches!(err, TextError::Fs(FsError::ExternalModification { .. })),
        "{err:?}"
    );
    assert!(!path.exists());
}
