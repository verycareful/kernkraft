// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! `same_filesystem` against real mounts: a place under the test directory
//! and `/proc` are on different filesystems (procfs is a mount of its own,
//! see proc_pid_mountinfo in proc(5)); two folders of the test directory are
//! on one; and a bind mount shares the device number of the folder it was
//! made from but is another mount, which rename(2) refuses to cross (EXDEV).
//! rename(2) decides by the folders the item leaves and enters, so the
//! answers are judged by the source's parent folder.
//!
//! One test, `btrfs_subvolume_files_and_roots`, is local only: a btrfs
//! subvolume is the case where the parent folder and the source itself have
//! different device numbers, and it needs the test directory on btrfs and
//! `btrfs subvolume create` (btrfs-progs, no privilege needed). It is
//! `#[ignore]`d so that `cargo test` on a machine without btrfs (CI runs on
//! ext4) does not run it; run it where the test directory (`TMPDIR`) is on
//! btrfs with `cargo test -p kernkraft-fs --test same_filesystem -- --ignored
//! btrfs_subvolume_files_and_roots`. Run anywhere else it fails, saying why;
//! it never skips quietly.
//!
//! The bind mount needs a mount namespace of its own. This process cannot
//! make one (unshare(2) with `CLONE_NEWUSER` refuses a process with more than
//! one thread, and the test harness has several), so the test starts this
//! same test binary under `unshare(1)`, which makes the bind mount and then
//! runs the test again as the child (the environment variables below tell
//! it so). Where unprivileged user namespaces are not allowed (the CI
//! runner's AppArmor policy), `KERN_TEST_SUDO_MOUNTNS=1` makes the namespace
//! with `sudo -n unshare --mount --propagation private` and drops back to the
//! invoking user with `setpriv(1)` before the test runs. A refused `unshare`
//! or `sudo` fails the test; nothing falls back to the other form.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::ffi::OsStr;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{symlink, MetadataExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use kernkraft_fs::{same_filesystem, FsError};

/// Set (to `1`) on CI to make the mount namespace with `sudo`.
const SUDO_ENV: &str = "KERN_TEST_SUDO_MOUNTNS";
/// Set in the child that runs inside the mount namespace: the folder the
/// sources are in, the folder on the same mount, and the bind mount of
/// another folder.
const CHILD_ENV: [&str; 3] = [
    "KERN_TEST_SAMEFS_SRC",
    "KERN_TEST_SAMEFS_SAME",
    "KERN_TEST_SAMEFS_BOUND",
];

struct Tree {
    _root: tempfile::TempDir,
    dir: PathBuf,
}

fn tree() -> Tree {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().to_path_buf();
    Tree { _root: root, dir }
}

fn file(dir: &Path, name: impl AsRef<OsStr>) -> PathBuf {
    let path = dir.join(name.as_ref());
    fs::write(&path, "x").unwrap();
    path
}

fn folder(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::create_dir(&path).unwrap();
    path
}

/// `answers` as `Some(same)` or `None` for an error, to compare in one go.
fn same(answers: &[Result<bool, FsError>]) -> Vec<Option<bool>> {
    answers.iter().map(|a| a.as_ref().ok().copied()).collect()
}

#[test]
fn folders_of_one_directory_are_on_one_filesystem() {
    let t = tree();
    let (a, b) = (folder(&t.dir, "a"), folder(&t.dir, "b"));
    let sources = [file(&a, "one"), file(&a, "two"), folder(&a, "sub")];

    let answers = same_filesystem(&sources, &b);

    // One answer per source, in the order given.
    assert_eq!(same(&answers), [Some(true), Some(true), Some(true)]);
}

#[test]
fn a_place_under_proc_is_another_filesystem() {
    let t = tree();
    let here = file(&t.dir, "here");
    // The premise: procfs has a device number of its own.
    let proc = Path::new("/proc");
    assert_ne!(
        fs::metadata(proc).unwrap().dev(),
        fs::metadata(&t.dir).unwrap().dev()
    );

    // From the test directory into /proc, and from /proc into it.
    assert_eq!(same(&same_filesystem(&[here], proc)), [Some(false)]);
    let version = PathBuf::from("/proc/version");
    assert_eq!(same(&same_filesystem(&[version], &t.dir)), [Some(false)]);
}

#[test]
fn each_source_is_judged_alone() {
    let t = tree();
    let dest = folder(&t.dir, "dest");
    let sources = [
        file(&t.dir, "here"),
        PathBuf::from("/proc/version"),
        file(&t.dir, "also-here"),
    ];

    assert_eq!(
        same(&same_filesystem(&sources, &dest)),
        [Some(true), Some(false), Some(true)]
    );
}

/// A symlink moves as itself, so the filesystem that counts is the one its
/// folder is on, not the one it points into.
#[test]
fn a_symlink_source_is_the_link_not_its_target() {
    let t = tree();
    let dest = folder(&t.dir, "dest");
    let into_proc = t.dir.join("into-proc");
    symlink("/proc/version", &into_proc).unwrap();
    // The premise: following the link would leave the test directory's
    // filesystem.
    assert_ne!(
        fs::metadata(&into_proc).unwrap().dev(),
        fs::metadata(&t.dir).unwrap().dev()
    );
    let dangling = t.dir.join("dangling");
    symlink(t.dir.join("not-there"), &dangling).unwrap();

    assert_eq!(
        same(&same_filesystem(&[into_proc, dangling], &dest)),
        [Some(true), Some(true)]
    );
}

/// The destination is where the move lands, so a link to a folder is that
/// folder.
#[test]
fn a_symlink_destination_is_followed() {
    let t = tree();
    let here = file(&t.dir, "here");
    let to_proc = t.dir.join("to-proc");
    symlink("/proc", &to_proc).unwrap();
    let to_dest = t.dir.join("to-dest");
    symlink(folder(&t.dir, "dest"), &to_dest).unwrap();

    assert_eq!(
        same(&same_filesystem(std::slice::from_ref(&here), &to_proc)),
        [Some(false)]
    );
    assert_eq!(same(&same_filesystem(&[here], &to_dest)), [Some(true)]);
}

#[test]
fn a_missing_source_fails_only_its_own_answer() {
    let t = tree();
    let dest = folder(&t.dir, "dest");
    let missing = t.dir.join("missing");
    let sources = [file(&t.dir, "a"), missing.clone(), file(&t.dir, "b")];

    let answers = same_filesystem(&sources, &dest);

    assert_eq!(answers.len(), 3);
    assert!(matches!(answers[0], Ok(true)));
    assert!(matches!(&answers[1], Err(FsError::NotFound { path }) if *path == missing));
    assert!(matches!(answers[2], Ok(true)));
}

#[test]
fn a_missing_destination_fails_every_answer() {
    let t = tree();
    let dest = t.dir.join("no-such-folder");
    let sources = [file(&t.dir, "a"), file(&t.dir, "b")];

    let answers = same_filesystem(&sources, &dest);

    assert_eq!(answers.len(), 2);
    for answer in &answers {
        assert!(
            matches!(answer, Err(FsError::NotFound { path }) if *path == dest),
            "{answer:?}"
        );
    }
}

/// A destination that cannot be looked at for a reason other than absence is
/// an I/O error naming it, not a missing folder.
#[test]
fn a_destination_below_a_file_is_an_io_error_with_its_path() {
    let t = tree();
    let blocker = file(&t.dir, "a-file");
    // A path through a file: ENOTDIR, which is not NotFound.
    let dest = blocker.join("below");
    let answers = same_filesystem(&[file(&t.dir, "a")], &dest);
    match &answers[..] {
        [Err(FsError::Io { path, source })] => {
            assert_eq!(*path, dest);
            assert_eq!(source.kind(), ErrorKind::NotADirectory);
        }
        other => panic!("{other:?}"),
    }
}

/// A path with no name in a folder, such as the root or one ending in `..`,
/// cannot be moved into anything.
#[test]
fn a_path_without_a_name_is_an_invalid_name() {
    let t = tree();
    let dest = folder(&t.dir, "dest");
    let up = folder(&t.dir, "sub").join("..");
    let answers = same_filesystem(&[PathBuf::from("/"), up], &dest);
    assert_eq!(answers.len(), 2);
    for answer in &answers {
        assert!(
            matches!(answer, Err(FsError::InvalidName { .. })),
            "{answer:?}"
        );
    }
}

#[test]
fn no_sources_give_no_answers() {
    let t = tree();
    assert!(same_filesystem(&[], &t.dir).is_empty());
}

#[test]
fn a_name_that_is_not_utf8_is_judged_by_its_bytes() {
    let t = tree();
    let dest = folder(&t.dir, "dest");
    let odd = file(&t.dir, OsStr::from_bytes(b"caf\xe9\xff.txt"));
    assert!(odd.to_str().is_none());

    assert_eq!(same(&same_filesystem(&[odd], &dest)), [Some(true)]);
}

/// A bind mount shares the device number of the folder it was made from but
/// is another mount, and rename(2) between mounts fails with EXDEV. The
/// answer must follow the rename, not the device number.
#[test]
fn a_bind_mount_is_another_filesystem() {
    let vars: Vec<Option<String>> = CHILD_ENV.iter().map(|v| std::env::var(v).ok()).collect();
    if let [Some(src), Some(same), Some(bound)] = &vars[..] {
        in_namespace(Path::new(src), Path::new(same), Path::new(bound));
        return;
    }
    run_in_namespace();
}

/// The parent: lays out the folders, then runs this test again in a new
/// mount namespace in which `bound` is a bind mount of `other`.
fn run_in_namespace() {
    let t = tree();
    let (src, same, other, bound) = (
        folder(&t.dir, "src"),
        folder(&t.dir, "same"),
        folder(&t.dir, "other"),
        folder(&t.dir, "bound"),
    );
    file(&src, "to-same");
    file(&src, "to-bound");
    let exe = std::env::current_exe().unwrap();
    let meta = fs::metadata(&t.dir).unwrap();
    let sudo = std::env::var(SUDO_ENV).as_deref() == Ok("1");

    // Makes the bind mount, then becomes the command that follows it.
    let script = r#"mount --bind "$1" "$2" || exit 3; shift 2; exec "$@""#;
    let mut cmd = if sudo {
        let mut c = Command::new("sudo");
        c.args(["-n", "unshare", "--mount", "--propagation", "private"]);
        c
    } else {
        let mut c = Command::new("unshare");
        c.args(["--user", "--map-root-user", "--mount"]);
        c
    };
    cmd.args(["sh", "-c", script, "sh"]).arg(&other).arg(&bound);
    if sudo {
        // The shell is root only to make the mount; the test runs as the
        // invoking user again.
        cmd.args([
            "setpriv".to_owned(),
            format!("--reuid={}", meta.uid()),
            format!("--regid={}", meta.gid()),
            "--clear-groups".to_owned(),
        ]);
    }
    cmd.arg("env");
    for (var, dir) in CHILD_ENV.iter().zip([&src, &same, &bound]) {
        let mut assignment = std::ffi::OsString::from(format!("{var}="));
        assignment.push(dir);
        cmd.arg(assignment);
    }
    cmd.arg(&exe).args([
        "--exact",
        "a_bind_mount_is_another_filesystem",
        "--nocapture",
    ]);

    let out = cmd
        .output()
        .expect("unshare(1) from util-linux (and sudo with KERN_TEST_SUDO_MOUNTNS=1) is needed");
    assert!(
        out.status.success(),
        "the test in the mount namespace failed ({})\nstdout:\n{}\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // The child ran the test body, not nothing: `--exact` with a name that
    // matches no test would also succeed.
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("1 passed"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// The child, inside the mount namespace: `src` holds `to-same` and
/// `to-bound`, `same` is a folder of the same mount, and `bound` is a bind
/// mount of another folder of it.
fn in_namespace(src: &Path, same_dir: &Path, bound: &Path) {
    // The bind mount has the device number of the mount it came from...
    assert_eq!(
        fs::metadata(src).unwrap().dev(),
        fs::metadata(bound).unwrap().dev()
    );
    // ...yet rename(2) refuses to cross into it, and works within the mount.
    let refused = fs::rename(src.join("to-bound"), bound.join("to-bound")).unwrap_err();
    assert_eq!(refused.kind(), ErrorKind::CrossesDevices, "{refused}");
    assert!(src.join("to-bound").exists());

    let to_same = src.join("to-same");
    let to_bound = src.join("to-bound");
    assert_eq!(
        same(&same_filesystem(&[to_same.clone(), to_bound], same_dir)),
        [Some(true), Some(true)]
    );
    assert_eq!(
        same(&same_filesystem(
            &[to_same.clone(), src.join("to-bound")],
            bound
        )),
        [Some(false), Some(false)]
    );
    // A source inside the bind mount is in that mount.
    let inside = file(bound, "inside");
    assert_eq!(same(&same_filesystem(&[inside], bound)), [Some(true)]);
    // And the rename the answer promised works.
    fs::rename(&to_same, same_dir.join("to-same")).unwrap();

    // rename(2) decides by the folders the item leaves and enters, so the
    // bind mount's own root is judged by the folder it sits in, which is in
    // the one mount with `same_dir`. (rename(2) does not move a mount point
    // at all, EBUSY and not EXDEV; the answer is about EXDEV.)
    assert_eq!(
        same(&same_filesystem(&[bound.to_path_buf()], same_dir)),
        [Some(true)]
    );
    let busy = fs::rename(bound, same_dir.join("moved")).unwrap_err();
    assert_eq!(busy.kind(), ErrorKind::ResourceBusy, "{busy}");
}

/// `BTRFS_SUPER_MAGIC`, the `f_type` of a btrfs filesystem (statfs(2)).
const BTRFS_SUPER_MAGIC: i64 = 0x9123_683e;

/// btrfs gives every subvolume a device number of its own and refuses to
/// rename an entry from one subvolume into another with EXDEV, but lets the
/// root of a subvolume move between folders (`btrfs_rename`). The folder the
/// item is in decides, so a file inside a subvolume cannot go to a folder of
/// the subvolume's parent while the subvolume's root can go from its parent
/// to a folder of it.
///
/// Local only (see the file header). The kernel is asked as well as
/// `same_filesystem`: the premise is checked with rename(2) itself.
#[test]
#[ignore = "local only: needs the test directory on btrfs and btrfs-progs; CI (ext4) does not run it"]
fn btrfs_subvolume_files_and_roots() {
    let t = tree();
    let kind = rustix::fs::statfs(&t.dir).unwrap().f_type;
    assert_eq!(
        kind,
        BTRFS_SUPER_MAGIC,
        "this test checks btrfs subvolumes and needs the test directory on btrfs: {} is on a \
         filesystem of type {kind:#x}, not btrfs ({BTRFS_SUPER_MAGIC:#x}). Point TMPDIR at a \
         folder on btrfs; it is local only and CI (ext4) does not run it",
        t.dir.display()
    );
    let parent = folder(&t.dir, "parent");
    let other = folder(&parent, "other");
    let sub = parent.join("sub");
    let made = Command::new("btrfs")
        .args(["subvolume", "create"])
        .arg(&sub)
        .output()
        .expect("this test needs `btrfs` (btrfs-progs) to make a subvolume");
    assert!(
        made.status.success(),
        "btrfs subvolume create {} failed ({}): {}",
        sub.display(),
        made.status,
        String::from_utf8_lossy(&made.stderr)
    );
    let inside = file(&sub, "inside");
    // The premise: the subvolume has a device number of its own, within the
    // one mount.
    assert_ne!(
        fs::metadata(&sub).unwrap().dev(),
        fs::metadata(&parent).unwrap().dev()
    );

    // A file inside the subvolume to a folder of its parent: rename(2)
    // refuses, and so the answer is "different".
    let refused = fs::rename(&inside, parent.join("inside")).unwrap_err();
    assert_eq!(refused.kind(), ErrorKind::CrossesDevices, "{refused}");
    assert_eq!(
        same(&same_filesystem(std::slice::from_ref(&inside), &other)),
        [Some(false)]
    );
    // And the other way: a file of the parent into the subvolume.
    let plain = file(&parent, "plain");
    assert_eq!(
        same(&same_filesystem(std::slice::from_ref(&plain), &sub)),
        [Some(false)]
    );

    // The subvolume's root within its parent: one folder to another of the
    // same subvolume, so "same", and rename(2) does it.
    assert_eq!(
        same(&same_filesystem(std::slice::from_ref(&sub), &other)),
        [Some(true)]
    );
    fs::rename(&sub, other.join("sub")).unwrap();

    // An empty subvolume can be removed like a folder, so the test directory
    // goes away with it.
    fs::remove_file(other.join("sub/inside")).unwrap();
    fs::remove_dir(other.join("sub")).unwrap();
}
