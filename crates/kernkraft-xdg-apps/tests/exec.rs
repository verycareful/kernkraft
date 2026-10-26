// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Desktop Entry Specification 1.5, "The Exec key".

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

use kernkraft_xdg_apps::{expand_exec, AppsError, DesktopEntry};

fn entry(exec: &str) -> DesktopEntry {
    DesktopEntry {
        id: "t.desktop".to_string(),
        name: "The App".to_string(),
        exec: exec.to_string(),
        icon: Some("t-icon".to_string()),
        mime_types: Vec::new(),
        terminal: false,
        no_display: false,
        try_exec: None,
        hidden: false,
        path: None,
        source: PathBuf::from("/apps/t.desktop"),
    }
}

fn files(names: &[&str]) -> Vec<PathBuf> {
    names.iter().map(PathBuf::from).collect()
}

fn strs(launches: &[Vec<OsString>]) -> Vec<Vec<String>> {
    launches
        .iter()
        .map(|l| l.iter().map(|a| a.to_str().unwrap().to_string()).collect())
        .collect()
}

#[test]
fn exec_percent_f_list_single_launch() {
    let out = expand_exec(&entry("app --x %F"), &files(&["/a", "/b c"])).unwrap();
    assert_eq!(strs(&out), [["app", "--x", "/a", "/b c"]]);
}

#[test]
fn exec_percent_f_one_launch_per_file() {
    let out = expand_exec(&entry("app %f"), &files(&["/a", "/b"])).unwrap();
    assert_eq!(strs(&out), [["app", "/a"], ["app", "/b"]]);
}

#[test]
fn no_files_removes_the_file_code() {
    let out = expand_exec(&entry("app %U"), &[]).unwrap();
    assert_eq!(strs(&out), [["app"]]);
    let out = expand_exec(&entry("app %f"), &[]).unwrap();
    assert_eq!(strs(&out), [["app"]]);
}

#[test]
fn exec_percent_u_file_uris_percent_encoded() {
    // Space, `%`, `#`, a multi-byte character and a byte (0xFF) that is not
    // UTF-8 each become %XX; unreserved characters and `/` stay.
    let mut raw = b"/d/a b%#-._~".to_vec();
    raw.extend("é".as_bytes());
    raw.push(0xFF);
    let path = PathBuf::from(OsString::from_vec(raw));
    let out = expand_exec(&entry("app %u"), std::slice::from_ref(&path)).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0][1], "file:///d/a%20b%25%23-._~%C3%A9%FF");

    // `%f` hands the path itself over, bytes unchanged.
    let out = expand_exec(&entry("app %f"), std::slice::from_ref(&path)).unwrap();
    assert_eq!(out[0][1], path.as_os_str());

    let out = expand_exec(&entry("app %U"), &files(&["/x y", "/z"])).unwrap();
    assert_eq!(strs(&out), [["app", "file:///x%20y", "file:///z"]]);
}

#[test]
fn exec_quoted_args_with_spaces() {
    let out = expand_exec(
        &entry(r#""/opt/my app/run" "a \"b\" \$HOME \\ c" plain %f"#),
        &files(&["/f"]),
    )
    .unwrap();
    assert_eq!(
        strs(&out),
        [["/opt/my app/run", r#"a "b" $HOME \ c"#, "plain", "/f"]]
    );
}

#[test]
fn exec_percent_percent_literal() {
    let out = expand_exec(&entry("app 100%% --n=%c"), &[]).unwrap();
    assert_eq!(strs(&out), [["app", "100%", "--n=The App"]]);
}

#[test]
fn icon_name_and_location_codes() {
    let out = expand_exec(&entry("app %i %c %k"), &[]).unwrap();
    assert_eq!(
        strs(&out),
        [["app", "--icon", "t-icon", "The App", "/apps/t.desktop"]]
    );
    let mut bare = entry("app %i");
    bare.icon = None;
    assert_eq!(strs(&expand_exec(&bare, &[]).unwrap()), [["app"]]);
}

#[test]
fn deprecated_codes_are_removed() {
    let out = expand_exec(&entry("app %d %D %n %N %v %m x"), &[]).unwrap();
    assert_eq!(strs(&out), [["app", "x"]]);
}

#[test]
fn exec_no_code_with_files_errors() {
    let err = expand_exec(&entry("app"), &files(&["/a"])).unwrap_err();
    assert!(matches!(err, AppsError::NoFileArgument { ref id } if id == "t.desktop"));
}

#[test]
fn invalid_exec_values() {
    for bad in [
        "app %z",
        "app \"unterminated",
        "app %f %F",
        "app x%F",
        "app %",
        "",
        "%d",
    ] {
        let r = expand_exec(&entry(bad), &[]);
        assert!(
            matches!(r, Err(AppsError::InvalidExec { .. })),
            "{bad:?}: {r:?}"
        );
    }
}

#[test]
fn field_code_as_program_rejected() {
    for bad in ["%f", "%u %U", "%F app", "\"%c\" x", "pre%f arg"] {
        let r = expand_exec(&entry(bad), &files(&["/a"]));
        assert!(
            matches!(r, Err(AppsError::InvalidExec { .. })),
            "{bad:?}: {r:?}"
        );
    }
    // Deprecated codes are removed first, so they are no obstacle.
    let out = expand_exec(&entry("%m app"), &[]).unwrap();
    assert_eq!(strs(&out), [["app"]]);
    // An escaped percent sign is text, not a code.
    let out = expand_exec(&entry("100%%app"), &[]).unwrap();
    assert_eq!(out[0][0], "100%app");
}
