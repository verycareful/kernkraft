// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use kernkraft_fs::{base_dirs_from, parse_user_dirs, user_dirs, BaseDirs};

fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    move |key| map.get(key).map(OsString::from)
}

fn base_with_config(config: &Path) -> BaseDirs {
    BaseDirs {
        home: PathBuf::from("/h"),
        config: config.to_path_buf(),
        cache: PathBuf::from("/h/.cache"),
        state: PathBuf::from("/h/.local/state"),
        data: PathBuf::from("/h/.local/share"),
    }
}

#[test]
fn relative_xdg_config_home_is_ignored() {
    let env = env_of(&[("HOME", "/h"), ("XDG_CONFIG_HOME", "rel/x")]);
    let dirs = base_dirs_from(&env).unwrap();
    assert_eq!(dirs.config, PathBuf::from("/h/.config"));
}

#[test]
fn empty_xdg_data_home_is_ignored() {
    let env = env_of(&[("HOME", "/h"), ("XDG_DATA_HOME", "")]);
    let dirs = base_dirs_from(&env).unwrap();
    assert_eq!(dirs.data, PathBuf::from("/h/.local/share"));
    assert_eq!(dirs.state, PathBuf::from("/h/.local/state"));
}

#[test]
fn absolute_xdg_cache_home_is_used() {
    let env = env_of(&[("HOME", "/h"), ("XDG_CACHE_HOME", "/c")]);
    let dirs = base_dirs_from(&env).unwrap();
    assert_eq!(dirs.cache, PathBuf::from("/c"));
    assert_eq!(dirs.home, PathBuf::from("/h"));
}

#[test]
fn missing_home_gives_none() {
    let env = env_of(&[("XDG_CACHE_HOME", "/c")]);
    assert!(base_dirs_from(&env).is_none());
}

#[test]
fn relative_home_gives_none() {
    let env = env_of(&[("HOME", "h")]);
    assert!(base_dirs_from(&env).is_none());
}

#[test]
fn parse_expands_leading_home() {
    let dirs = parse_user_dirs("XDG_DOCUMENTS_DIR=\"$HOME/Dokumente\"\n", Path::new("/h"));
    assert_eq!(dirs["DOCUMENTS"], PathBuf::from("/h/Dokumente"));
}

#[test]
fn parse_expands_bare_home() {
    let dirs = parse_user_dirs("XDG_DESKTOP_DIR=\"$HOME\"\n", Path::new("/h"));
    assert_eq!(dirs["DESKTOP"], PathBuf::from("/h"));
}

#[test]
fn parse_keeps_absolute_value() {
    let dirs = parse_user_dirs("XDG_DOWNLOAD_DIR=\"/data/dl\"\n", Path::new("/h"));
    assert_eq!(dirs["DOWNLOAD"], PathBuf::from("/data/dl"));
}

#[test]
fn parse_skips_comments_and_garbage() {
    let dirs = parse_user_dirs(
        "# XDG_PICTURES_DIR=\"$HOME/p\"\nXDG_MUSIC_DIR=$HOME/m\nnonsense\n",
        Path::new("/h"),
    );
    assert!(dirs.is_empty(), "{dirs:?}");
}

#[test]
fn parse_skips_relative_result() {
    let dirs = parse_user_dirs("XDG_VIDEOS_DIR=\"videos\"\n", Path::new("/h"));
    assert!(!dirs.contains_key("VIDEOS"));
}

#[test]
fn parse_does_not_expand_home_mid_value() {
    let dirs = parse_user_dirs("XDG_MUSIC_DIR=\"/x/$HOME\"\n", Path::new("/h"));
    assert_eq!(dirs["MUSIC"], PathBuf::from("/x/$HOME"));
}

#[test]
fn user_dirs_fall_back_to_english_names() {
    let config = tempfile::tempdir().unwrap();
    let dirs = user_dirs(&base_with_config(config.path())).unwrap();
    assert_eq!(dirs.documents, PathBuf::from("/h/Documents"));
    assert_eq!(dirs.download, PathBuf::from("/h/Downloads"));
    assert_eq!(dirs.videos, PathBuf::from("/h/Videos"));
}

#[test]
fn user_dirs_read_config_file() {
    let config = tempfile::tempdir().unwrap();
    std::fs::write(
        config.path().join("user-dirs.dirs"),
        "XDG_PICTURES_DIR=\"$HOME/Bilder\"\n",
    )
    .unwrap();
    let dirs = user_dirs(&base_with_config(config.path())).unwrap();
    assert_eq!(dirs.pictures, PathBuf::from("/h/Bilder"));
    assert_eq!(dirs.music, PathBuf::from("/h/Music"));
}

#[test]
fn user_dirs_reject_non_utf8_file() {
    let config = tempfile::tempdir().unwrap();
    std::fs::write(
        config.path().join("user-dirs.dirs"),
        b"XDG_MUSIC_DIR=\"/\xff\"\n",
    )
    .unwrap();
    let err = user_dirs(&base_with_config(config.path())).unwrap_err();
    assert!(matches!(err, kernkraft_fs::FsError::Io { .. }), "{err:?}");
}

/// `user-dirs.dirs` is shell assignments: a later line wins.
#[test]
fn user_dirs_last_duplicate_wins() {
    let parsed = parse_user_dirs(
        "XDG_DOCUMENTS_DIR=\"$HOME/one\"\nXDG_DOCUMENTS_DIR=\"$HOME/two\"\n",
        Path::new("/h"),
    );
    assert_eq!(parsed["DOCUMENTS"], PathBuf::from("/h/two"));
}

#[test]
fn user_dirs_home_alone_and_indented_lines() {
    let parsed = parse_user_dirs("   XDG_DESKTOP_DIR=\"$HOME\"\n\t# c\n", Path::new("/h"));
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed["DESKTOP"], PathBuf::from("/h"));
}
