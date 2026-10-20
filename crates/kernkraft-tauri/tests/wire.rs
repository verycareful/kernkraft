// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use kernkraft_fs::{Change, FsError, IrreversibleReason};
use kernkraft_tauri::{WireError, WirePath, WirePathError};

fn p(s: &str) -> PathBuf {
    PathBuf::from(s)
}

#[test]
fn wire_path_round_trips_non_utf8() {
    let original = PathBuf::from(OsStr::from_bytes(b"a\xffb"));
    let wire = WirePath::from(original.as_path());
    assert!(matches!(wire, WirePath::Bytes(_)));
    let json = serde_json::to_string(&wire).unwrap();
    let back: WirePath = serde_json::from_str(&json).unwrap();
    assert_eq!(PathBuf::try_from(back).unwrap(), original);
}

#[test]
fn wire_path_utf8_is_plain_string() {
    let wire = WirePath::from(Path::new("/home/a b/é"));
    assert_eq!(wire, WirePath::Utf8("/home/a b/é".to_owned()));
    let json = serde_json::to_value(&wire).unwrap();
    assert_eq!(json, serde_json::json!({"k": "Utf8", "v": "/home/a b/é"}));
    assert_eq!(PathBuf::try_from(wire).unwrap(), p("/home/a b/é"));
}

#[test]
fn wire_path_bytes_json_shape() {
    // base64 (RFC 4648 standard, padded) of the bytes 61 FF 62 is "Yf9i".
    let wire = WirePath::from(Path::new(OsStr::from_bytes(b"a\xffb")));
    assert_eq!(
        serde_json::to_value(&wire).unwrap(),
        serde_json::json!({"k": "Bytes", "v": "Yf9i"})
    );
}

#[test]
fn wire_path_decode_rejects_untrusted_input() {
    assert!(matches!(
        PathBuf::try_from(WirePath::Bytes("not base64!".into())),
        Err(WirePathError::BadBase64(_))
    ));
    // "YQBi" is the bytes 61 00 62.
    assert_eq!(
        PathBuf::try_from(WirePath::Bytes("YQBi".into())),
        Err(WirePathError::Nul)
    );
    assert_eq!(
        PathBuf::try_from(WirePath::Utf8("a\0b".into())),
        Err(WirePathError::Nul)
    );
}

#[test]
fn wire_path_display_is_lossy() {
    let wire = WirePath::from(Path::new(OsStr::from_bytes(b"a\xffb")));
    assert_eq!(wire.display(), "a\u{fffd}b");
    assert_eq!(WirePath::Utf8("/x".into()).display(), "/x");
}

#[test]
fn fs_error_kind_names() {
    let io = || std::io::Error::from(std::io::ErrorKind::Other);
    let cases: Vec<(FsError, &str, bool)> = vec![
        (
            FsError::Io {
                path: p("/a"),
                source: io(),
            },
            "Io",
            true,
        ),
        (
            FsError::ExternalModification { path: p("/a") },
            "ExternalModification",
            true,
        ),
        (FsError::Exists { path: p("/a") }, "Exists", true),
        (FsError::NotFound { path: p("/a") }, "NotFound", true),
        (
            FsError::InvalidName { name: "a/b".into() },
            "InvalidName",
            false,
        ),
        (FsError::IntoItself { src: p("/a") }, "IntoItself", true),
        (
            FsError::TrashUnavailable { path: p("/a") },
            "TrashUnavailable",
            true,
        ),
        (
            FsError::BadTrashInfo { path: p("/a") },
            "BadTrashInfo",
            true,
        ),
        (
            FsError::UnsafeCacheDir { path: p("/a") },
            "UnsafeCacheDir",
            true,
        ),
        (
            FsError::OwnershipNotKept { path: p("/a") },
            "OwnershipNotKept",
            true,
        ),
        (
            FsError::XattrsNotKept {
                path: p("/a"),
                names: vec!["user.x".into()],
            },
            "XattrsNotKept",
            true,
        ),
        (FsError::Cancelled, "Cancelled", false),
        (
            FsError::Changed {
                path: p("/a"),
                what: Change::Missing,
            },
            "Changed",
            true,
        ),
        (
            FsError::Irreversible {
                what: "delete".into(),
                reason: IrreversibleReason::ReplacedExisting,
            },
            "Irreversible",
            false,
        ),
        (
            FsError::Partial {
                done: vec![p("/a")],
                not_done: vec![p("/b")],
                cause: Box::new(FsError::Cancelled),
                warnings: Vec::new(),
            },
            "Partial",
            false,
        ),
        (
            FsError::SourceNotRemoved {
                copy: p("/c"),
                left: p("/l"),
                cause: Box::new(FsError::Cancelled),
            },
            "SourceNotRemoved",
            true,
        ),
        (FsError::NothingToUndo, "NothingToUndo", false),
        (FsError::NothingToRedo, "NothingToRedo", false),
    ];
    for (err, kind, has_path) in cases {
        let message = err.to_string();
        let wire = WireError::from(err);
        assert_eq!(wire.kind, kind);
        assert_eq!(wire.message, message);
        assert_eq!(wire.path.is_some(), has_path, "{kind}");
    }
}

#[test]
fn wire_error_carries_non_utf8_path() {
    let path = PathBuf::from(OsStr::from_bytes(b"x\xfe"));
    let wire = WireError::from(FsError::Exists { path: path.clone() });
    let json = serde_json::to_string(&wire).unwrap();
    let back: WireError = serde_json::from_str(&json).unwrap();
    assert_eq!(PathBuf::try_from(back.path.unwrap()).unwrap(), path);
}

#[test]
fn wrapped_errors_report_the_inner_kind() {
    use kernkraft_text::TextError;
    let wire = WireError::from(TextError::Fs(FsError::NotFound { path: p("/a") }));
    assert_eq!(wire.kind, "NotFound");
    assert_eq!(wire.path, Some(WirePath::Utf8("/a".into())));
    let wire = WireError::from(TextError::TooLargeToOpen { len: 2, limit: 1 });
    assert_eq!(wire.kind, "TooLargeToOpen");
}

#[test]
fn plugin_builds_with_the_mock_runtime() {
    let app = tauri::test::mock_builder()
        .plugin(kernkraft_tauri::init())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    drop(app);
}
