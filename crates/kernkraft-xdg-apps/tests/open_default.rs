// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `open_default`: files grouped by their default application, one launch per
//! application, one outcome per application. Applications are stubs that
//! record what they were given; the tree and the file types are built here,
//! so every expectation derives from how the input was built.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kernkraft_fs::{base_dirs_from, BaseDirs};
use kernkraft_xdg_apps::{default_groups, open_default, AppRegistry, AppsError, MimeDb};

struct Tree {
    dir: tempfile::TempDir,
}

impl Tree {
    /// Types `*.txt` as text/plain, `*.png` as image/png and `*.pdf` as
    /// application/pdf; `text.desktop` and `image.desktop` record their
    /// arguments in `<name>.log`; `broken.desktop` (for PDFs) names a program
    /// that does not exist; `*.xyz` is typed by content.
    fn new() -> Tree {
        let t = Tree {
            dir: tempfile::tempdir().unwrap(),
        };
        t.write(
            "data/mime/globs2",
            "50:text/plain:*.txt\n50:image/png:*.png\n50:application/pdf:*.pdf\n",
        );
        for (id, mime) in [("text", "text/plain"), ("image", "image/png")] {
            let script = t.path(&format!("{id}.sh"));
            fs::write(&script, "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$0.log\"\n").unwrap();
            t.write(
                &format!("data/applications/{id}.desktop"),
                &format!(
                    "[Desktop Entry]\nType=Application\nName={id}\nExec=/bin/sh {} %F\nMimeType={mime};\n",
                    script.display()
                ),
            );
        }
        t.write(
            "data/applications/broken.desktop",
            "[Desktop Entry]\nType=Application\nName=broken\nExec=/definitely/not/a/program %F\nMimeType=application/pdf;\n",
        );
        t.write(
            "config/mimeapps.list",
            "[Default Applications]\ntext/plain=text.desktop\nimage/png=image.desktop\napplication/pdf=broken.desktop\n",
        );
        t
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.path(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn file(&self, name: &str) -> PathBuf {
        let p = self.path(&format!("files/{name}"));
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, "x").unwrap();
        p
    }

    fn base(&self) -> BaseDirs {
        let root = self.dir.path();
        let mut vars: HashMap<&str, OsString> = HashMap::new();
        vars.insert("HOME", root.into());
        vars.insert("XDG_CONFIG_HOME", root.join("config").into());
        vars.insert("XDG_DATA_HOME", root.join("data").into());
        vars.insert("XDG_CONFIG_DIRS", root.join("none").into());
        vars.insert("XDG_DATA_DIRS", root.join("none").into());
        base_dirs_from(&|k| vars.get(k).cloned()).unwrap()
    }

    fn db(&self) -> MimeDb {
        MimeDb::load(&[self.path("data")]).unwrap()
    }

    /// The lines the stub `id` recorded, once there are `count` of them.
    fn recorded(&self, id: &str, count: usize) -> Vec<String> {
        let log = self.path(&format!("{id}.sh.log"));
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(text) = fs::read_to_string(&log) {
                if text.lines().count() >= count {
                    return text.lines().map(str::to_owned).collect();
                }
            }
            assert!(Instant::now() < deadline, "{id} never recorded {count}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn ids(groups: &[kernkraft_xdg_apps::DefaultGroup<'_>]) -> Vec<String> {
    groups.iter().map(|g| g.app.id.clone()).collect()
}

fn paths(p: &[&Path]) -> Vec<PathBuf> {
    p.iter().map(|p| p.to_path_buf()).collect()
}

#[test]
fn files_are_grouped_by_default_app_in_order_of_first_appearance() {
    let t = Tree::new();
    let (a, p, b) = (t.file("a.txt"), t.file("p.png"), t.file("b.txt"));
    let registry = AppRegistry::load(&t.base()).unwrap();
    let groups = default_groups(&registry, &t.db(), &paths(&[&a, &p, &b])).unwrap();
    assert_eq!(ids(&groups), ["text.desktop", "image.desktop"]);
    assert_eq!(groups[0].paths, paths(&[&a, &b]));
    assert_eq!(groups[1].paths, paths(&[&p]));
}

#[test]
fn a_file_without_a_default_starts_nothing_and_is_named() {
    let t = Tree::new();
    let a = t.file("a.txt");
    // No name pattern, and NUL bytes, so application/octet-stream (see
    // MimeDb::guess), which no application is the default for.
    let nothing = t.path("files/what.xyz");
    fs::write(&nothing, b"\0\0\0").unwrap();
    let registry = AppRegistry::load(&t.base()).unwrap();
    let err = open_default(&registry, &t.db(), &paths(&[&a, &nothing]), None).unwrap_err();
    match err {
        AppsError::NoDefaultApp { path, mime } => {
            assert_eq!(path, nothing);
            assert_eq!(mime, "application/octet-stream");
        }
        other => panic!("{other:?}"),
    }
    // Nothing was started for the file before it.
    std::thread::sleep(Duration::from_millis(300));
    assert!(!t.path("text.sh.log").exists());
}

#[test]
fn each_application_has_its_own_outcome_and_a_failure_stops_no_other() {
    let t = Tree::new();
    // The failing application comes first; the ones after it still start.
    let (pdf, a, p, b) = (
        t.file("doc.pdf"),
        t.file("a.txt"),
        t.file("p.png"),
        t.file("b.txt"),
    );
    let registry = AppRegistry::load(&t.base()).unwrap();
    let outcomes = open_default(&registry, &t.db(), &paths(&[&pdf, &a, &p, &b]), None).unwrap();

    let apps: Vec<&str> = outcomes.iter().map(|o| o.app.as_str()).collect();
    assert_eq!(apps, ["broken.desktop", "text.desktop", "image.desktop"]);
    assert!(matches!(
        &outcomes[0].result,
        Err(AppsError::Launch { id, .. }) if id == "broken.desktop"
    ));
    assert_eq!(outcomes[0].paths, paths(&[&pdf]));
    assert!(outcomes[1].result.is_ok());
    assert_eq!(outcomes[1].paths, paths(&[&a, &b]));
    assert!(outcomes[2].result.is_ok());
    assert_eq!(outcomes[2].paths, paths(&[&p]));

    // The two that started got exactly their files.
    let to_text = t.recorded("text", 2);
    assert_eq!(
        to_text,
        [
            a.to_str().unwrap().to_owned(),
            b.to_str().unwrap().to_owned()
        ]
    );
    assert_eq!(t.recorded("image", 1), [p.to_str().unwrap().to_owned()]);
}

#[test]
fn no_paths_give_no_outcomes() {
    let t = Tree::new();
    let registry = AppRegistry::load(&t.base()).unwrap();
    assert!(open_default(&registry, &t.db(), &[], None)
        .unwrap()
        .is_empty());
}
