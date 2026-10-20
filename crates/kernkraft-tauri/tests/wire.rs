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

fn io() -> std::io::Error {
    std::io::Error::from(std::io::ErrorKind::Other)
}

fn check<E: std::fmt::Display>(cases: Vec<(E, &str, bool)>, to_wire: impl Fn(E) -> WireError) {
    for (err, kind, has_path) in cases {
        let message = err.to_string();
        let wire = to_wire(err);
        assert_eq!(wire.kind, kind);
        assert_eq!(wire.message, message, "{kind}");
        assert_eq!(wire.path.is_some(), has_path, "{kind}");
    }
}

#[test]
fn apps_error_kind_names() {
    use kernkraft_xdg_apps::AppsError;
    let cases = vec![
        (
            AppsError::Io {
                path: p("/a"),
                source: io(),
            },
            "Io",
            true,
        ),
        (
            AppsError::MalformedMimeDb {
                path: p("/a"),
                line: 1,
            },
            "MalformedMimeDb",
            true,
        ),
        (
            AppsError::InvalidDesktopEntry {
                path: p("/a"),
                reason: "r".into(),
            },
            "InvalidDesktopEntry",
            true,
        ),
        (
            AppsError::NotAnApplication {
                path: p("/a"),
                kind: "Link".into(),
            },
            "NotAnApplication",
            true,
        ),
        (
            AppsError::MalformedMimeApps {
                path: p("/a"),
                line: 1,
            },
            "MalformedMimeApps",
            true,
        ),
        (
            AppsError::InvalidExec {
                id: "x".into(),
                reason: "r".into(),
            },
            "InvalidExec",
            false,
        ),
        (
            AppsError::NoFileArgument { id: "x".into() },
            "NoFileArgument",
            false,
        ),
        (
            AppsError::Launch {
                id: "x".into(),
                source: io(),
            },
            "Launch",
            false,
        ),
        (
            AppsError::XdgMime {
                status: std::os::unix::process::ExitStatusExt::from_raw(256),
                stderr: String::new(),
            },
            "XdgMime",
            false,
        ),
        (AppsError::NoTerminal { tried: vec![] }, "NoTerminal", false),
        (
            AppsError::InvalidArgument {
                what: "w",
                value: "v".into(),
            },
            "InvalidArgument",
            false,
        ),
    ];
    check(cases, WireError::from);
}

#[test]
fn thumb_error_kind_names() {
    use kernkraft_thumb::ThumbError;
    let cases = vec![
        (
            ThumbError::Io {
                path: p("/a"),
                source: io(),
            },
            "Io",
            true,
        ),
        (
            ThumbError::Decode {
                path: p("/a"),
                reason: "r".into(),
            },
            "Decode",
            true,
        ),
        (
            ThumbError::Unsupported { mime: "a/b".into() },
            "Unsupported",
            false,
        ),
        // Forwards: the inner kind and path.
        (
            ThumbError::Fs(FsError::Exists { path: p("/a") }),
            "Exists",
            true,
        ),
    ];
    check(cases, WireError::from);
}

#[test]
fn theme_error_kind_names() {
    use kernkraft_theme::ThemeError;
    let cases = vec![
        (
            ThemeError::Io {
                path: p("/a"),
                source: io(),
            },
            "Io",
            true,
        ),
        (
            ThemeError::TooLarge {
                path: p("/a"),
                limit: 1,
            },
            "TooLarge",
            true,
        ),
        (ThemeError::Portal("x".into()), "Portal", false),
        (
            ThemeError::Watch {
                path: p("/a"),
                source: io(),
            },
            "Watch",
            true,
        ),
        (
            ThemeError::CallbackPanic("x".into()),
            "CallbackPanic",
            false,
        ),
    ];
    check(cases, WireError::from);
}

#[test]
fn bus_error_kind_names() {
    use kernkraft_bus::BusError;
    let cases = vec![
        (
            BusError::NoAddress("DBUS_SESSION_BUS_ADDRESS"),
            "NoAddress",
            false,
        ),
        (BusError::NotUnix("tcp:".into()), "NotUnix", false),
        (
            BusError::Malformed {
                address: "x".into(),
                reason: "r",
            },
            "Malformed",
            false,
        ),
        (
            BusError::Connect {
                target: "t".into(),
                defaulted: false,
                source: io(),
            },
            "Connect",
            false,
        ),
    ];
    check(cases, WireError::from);
}

#[test]
fn volumes_error_kind_names() {
    use kernkraft_bus::BusError;
    use kernkraft_volumes::{EjectRefusal, VolumesError};
    let cases = vec![
        (
            VolumesError::Io {
                path: p("/a"),
                source: io(),
            },
            "Io",
            true,
        ),
        (
            VolumesError::MalformedMountinfo {
                line: 1,
                reason: "r",
            },
            "MalformedMountinfo",
            false,
        ),
        (VolumesError::NoUser { uid: 1 }, "NoUser", false),
        // Forwards: the inner bus error's kind.
        (
            VolumesError::Bus(BusError::NotUnix("tcp:".into())),
            "NotUnix",
            false,
        ),
        (
            VolumesError::Busy {
                mount_point: p("/m"),
                message: "m".into(),
            },
            "Busy",
            true,
        ),
        (
            VolumesError::NotAuthorized {
                message: "m".into(),
                dismissed: true,
            },
            "NotAuthorized",
            false,
        ),
        (
            VolumesError::NotMounted {
                mount_point: p("/m"),
            },
            "NotMounted",
            true,
        ),
        (
            VolumesError::NoEject {
                mount_point: p("/m"),
            },
            "NoEject",
            true,
        ),
        (
            VolumesError::EjectRefused {
                mount_point: p("/m"),
                reason: EjectRefusal::NoDrive,
            },
            "EjectRefused",
            true,
        ),
        (
            VolumesError::EjectIncomplete {
                unmounted: vec![p("/m")],
                locked: vec![],
                failed: p("/n"),
                cause: Box::new(VolumesError::NoUser { uid: 1 }),
            },
            "EjectIncomplete",
            true,
        ),
        (VolumesError::Timeout { what: "w".into() }, "Timeout", false),
        (
            VolumesError::Udisks {
                what: "w".into(),
                name: "n".into(),
                message: "m".into(),
            },
            "Udisks",
            false,
        ),
        (
            VolumesError::Dbus {
                what: "w".into(),
                reason: "r".into(),
            },
            "Dbus",
            false,
        ),
    ];
    check(cases, WireError::from);
}

#[test]
fn text_and_path_error_kind_names() {
    use kernkraft_text::TextError;
    check(
        vec![
            (
                TextError::Fs(FsError::NotFound { path: p("/a") }),
                "NotFound",
                true,
            ),
            (
                TextError::TooLargeToOpen { len: 2, limit: 1 },
                "TooLargeToOpen",
                false,
            ),
        ],
        WireError::from,
    );
    check(
        vec![
            (WirePathError::BadBase64("x".into()), "BadBase64", false),
            (WirePathError::Nul, "Nul", false),
        ],
        WireError::from,
    );
}

#[test]
fn plugin_error_kind_names() {
    use kernkraft_tauri::PluginError;
    let cases = vec![
        (PluginError::Busy("x"), "Busy", false),
        (PluginError::UnknownOp(1), "UnknownOp", false),
        (
            PluginError::UnknownConflict { op: 1, conflict: 2 },
            "UnknownConflict",
            false,
        ),
        (PluginError::UnknownWatch("k".into()), "UnknownWatch", false),
        (
            PluginError::InvalidArgument {
                what: "w",
                value: "v".into(),
            },
            "InvalidArgument",
            false,
        ),
        (PluginError::NoHome, "NoHome", false),
        (PluginError::NotInTrash(p("/a")), "NotInTrash", true),
        (
            PluginError::Watch {
                path: p("/a"),
                source: io(),
            },
            "Watch",
            true,
        ),
        (PluginError::Internal("x".into()), "Internal", false),
    ];
    check(cases, WireError::from);
}

#[test]
fn forwarding_variants_keep_the_outer_message() {
    use kernkraft_bus::BusError;
    use kernkraft_volumes::VolumesError;
    let e = VolumesError::Bus(BusError::NotUnix("tcp:".into()));
    let message = e.to_string();
    let wire = WireError::from(e);
    assert_eq!(wire.kind, "NotUnix");
    assert_eq!(wire.message, message);
    assert!(wire.message.starts_with("system bus: "));
}

#[test]
fn partial_reaches_the_ui_item_by_item() {
    let bad = PathBuf::from(OsStr::from_bytes(b"x\xfe"));
    let wire = WireError::from(FsError::Partial {
        done: vec![p("/a"), bad.clone()],
        not_done: vec![p("/b")],
        cause: Box::new(FsError::NotFound { path: p("/b") }),
        warnings: vec![(
            p("/a"),
            FsError::XattrsNotKept {
                path: p("/a"),
                names: vec!["user.x".into()],
            },
        )],
    });
    let json = serde_json::to_value(&wire).unwrap();
    assert_eq!(json["details"]["k"], "Partial");
    assert_eq!(
        json["details"]["done"][0],
        serde_json::json!({"k": "Utf8", "v": "/a"})
    );
    assert_eq!(json["details"]["done"][1]["k"], "Bytes");
    assert_eq!(json["details"]["not_done"][0]["v"], "/b");
    assert_eq!(json["details"]["cause"]["kind"], "NotFound");
    assert_eq!(
        json["details"]["warnings"][0]["error"]["kind"],
        "XattrsNotKept"
    );
    assert_eq!(
        json["details"]["warnings"][0]["error"]["details"],
        serde_json::json!({"k": "XattrsNotKept", "names": ["user.x"]})
    );
    let back: WireError = serde_json::from_value(json).unwrap();
    assert_eq!(back, wire);
}

#[test]
fn source_not_removed_and_changed_details() {
    let wire = WireError::from(FsError::SourceNotRemoved {
        copy: p("/c"),
        left: p("/l"),
        cause: Box::new(FsError::Cancelled),
    });
    assert_eq!(
        serde_json::to_value(wire.details.unwrap()).unwrap(),
        serde_json::json!({
            "k": "SourceNotRemoved",
            "copy": {"k": "Utf8", "v": "/c"},
            "left": {"k": "Utf8", "v": "/l"},
            "cause": {"kind": "Cancelled", "message": "cancelled", "path": null, "details": null},
        })
    );
    let wire = WireError::from(FsError::Changed {
        path: p("/a"),
        what: Change::Taken,
    });
    assert_eq!(
        serde_json::to_value(wire.details.unwrap()).unwrap(),
        serde_json::json!({"k": "Changed", "what": "Taken"})
    );
    let plain = WireError::from(FsError::Exists { path: p("/a") });
    assert_eq!(plain.details, None);
}

#[test]
fn eject_incomplete_details() {
    use kernkraft_volumes::VolumesError;
    let wire = WireError::from(VolumesError::EjectIncomplete {
        unmounted: vec![p("/m")],
        locked: vec![p("/org/freedesktop/UDisks2/block_devices/dm_2d0")],
        failed: p("/n"),
        cause: Box::new(VolumesError::Timeout {
            what: "eject".into(),
        }),
    });
    let json = serde_json::to_value(wire.details.unwrap()).unwrap();
    assert_eq!(json["k"], "EjectIncomplete");
    assert_eq!(json["unmounted"][0]["v"], "/m");
    assert_eq!(json["failed"]["v"], "/n");
    assert_eq!(json["cause"]["kind"], "Timeout");
}
