// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Desktop Entry Specification 1.5: "Basic format of the file", "Possible
//! value types", "Localized values for keys", "Desktop File ID".

use std::path::{Path, PathBuf};

use kernkraft_xdg_apps::{desktop_id, parse_desktop_entry, AppsError};

fn src() -> PathBuf {
    PathBuf::from("/a/applications/x.desktop")
}

#[test]
fn parses_escapes_and_lists() {
    let text = "[Desktop Entry]\nType=Application\nName=My\\sApp\\\\x\nExec=app --opt=a\\sb %F\n\
                MimeType=text/plain;image/png;a\\;b/c;\nIcon=app-icon\nTerminal=true\n\
                NoDisplay=true\nHidden=false\nPath=/work dir\n";
    let e = parse_desktop_entry("x.desktop", &src(), text).unwrap();
    assert_eq!(e.id, "x.desktop");
    assert_eq!(e.name, "My App\\x");
    assert_eq!(e.exec, "app --opt=a b %F");
    // The trailing `;` leaves no empty element; `\;` is a literal semicolon.
    assert_eq!(e.mime_types, ["text/plain", "image/png", "a;b/c"]);
    assert_eq!(e.icon.as_deref(), Some("app-icon"));
    assert!(e.terminal && e.no_display && !e.hidden);
    assert_eq!(e.path, Some(PathBuf::from("/work dir")));
    assert_eq!(e.source, src());
}

#[test]
fn ignores_other_groups() {
    let text = "[Desktop Entry]\nType=Application\nName=Real\nExec=real\n\
                [Desktop Action new]\nName=Action\nExec=action\n\
                [X-Other]\nExec=other\nhas no equals sign\n";
    let e = parse_desktop_entry("x.desktop", &src(), text).unwrap();
    assert_eq!((e.name.as_str(), e.exec.as_str()), ("Real", "real"));
}

#[test]
fn localized_name_not_used() {
    let text = "[Desktop Entry]\nType=Application\nName[de]=Deutsch\nName=Plain\n\
                Name[fr]=Francais\nExec=x\n";
    let e = parse_desktop_entry("x.desktop", &src(), text).unwrap();
    assert_eq!(e.name, "Plain");
    // With only a localized name, the ID stands in.
    let only = "[Desktop Entry]\nType=Application\nName[de]=Deutsch\nExec=x\n";
    assert_eq!(
        parse_desktop_entry("x.desktop", &src(), only).unwrap().name,
        "x.desktop"
    );
}

#[test]
fn desktop_id_from_subdir() {
    let dir = Path::new("/usr/share/applications");
    assert_eq!(
        desktop_id(dir, &dir.join("kde/foo.desktop")),
        "kde-foo.desktop"
    );
    assert_eq!(desktop_id(dir, &dir.join("a/b/c.desktop")), "a-b-c.desktop");
    assert_eq!(desktop_id(dir, &dir.join("plain.desktop")), "plain.desktop");
}

#[test]
fn rejects_what_is_not_a_usable_application() {
    let p = |t: &str| parse_desktop_entry("x.desktop", &src(), t);
    assert!(matches!(
        p("[Desktop Entry]\nType=Link\nName=L\nURL=u\n"),
        Err(AppsError::NotAnApplication { .. })
    ));
    for bad in [
        "Type=Application\nExec=x\n",
        "[Desktop Entry]\nExec=x\n",
        "[Desktop Entry]\nType=Application\n",
        "[Desktop Entry]\nType=Application\nExec=x\nTerminal=maybe\n",
        "[Desktop Entry]\nType=Application\nExec=x\nnonsense\n",
        "[Other]\nType=Application\nExec=x\n",
    ] {
        assert!(
            matches!(p(bad), Err(AppsError::InvalidDesktopEntry { .. })),
            "{bad:?}"
        );
    }
}
