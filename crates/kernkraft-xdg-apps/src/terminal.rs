// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Finding a terminal emulator and building the command that opens it.
//!
//! There is no freedesktop.org specification for "the" terminal. Sources, in
//! order, and how each is invoked with a program to run:
//! 1. `xdg-terminal-exec` on `PATH` (the xdg-terminal-exec proposal): the
//!    program and its arguments follow directly, `xdg-terminal-exec <argv>`.
//! 2. `$TERMINAL`: a terminal emulator by name or path (the convention of
//!    i3-sensible-terminal and similar launchers), run as `$TERMINAL -e
//!    <argv>`.
//! 3. The caller's setting (Kern's own preference), run as `<setting> -e
//!    <argv>`.
//!
//! Each of `$TERMINAL` and the setting is one program, not a command line;
//! `-e` is added only when there is a program to run. The terminal starts in
//! the directory given, through the child's working directory, so no
//! emulator-specific option is needed.

use std::ffi::{OsStr, OsString};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::AppsError;
use crate::launch::spawn_detached;

const XDG_TERMINAL_EXEC: &str = "xdg-terminal-exec";

/// The command that opens a terminal in `dir`, from the process environment.
/// `setting` is the caller's configured terminal. See the module docs for
/// the order and conventions.
pub fn terminal_command(dir: &Path, setting: Option<&str>) -> Result<Command, AppsError> {
    terminal_command_for(&|v| std::env::var_os(v), dir, setting, &[])
}

/// As [`terminal_command`], reading `PATH` and `TERMINAL` through `env` and
/// running `argv` (program first) in the terminal; an empty `argv` opens a
/// shell. The command's working directory is `dir`.
pub fn terminal_command_for(
    env: &dyn Fn(&str) -> Option<OsString>,
    dir: &Path,
    setting: Option<&str>,
    argv: &[OsString],
) -> Result<Command, AppsError> {
    let path = env("PATH");
    let mut tried = Vec::new();

    if let Some(found) = find_program(OsStr::new(XDG_TERMINAL_EXEC), path.as_deref()) {
        let mut cmd = Command::new(found);
        cmd.args(argv).current_dir(dir);
        return Ok(cmd);
    }
    tried.push(format!("{XDG_TERMINAL_EXEC} (not found on PATH)"));

    let candidates = [
        ("$TERMINAL", env("TERMINAL").filter(|t| !t.is_empty())),
        (
            "setting",
            setting.filter(|s| !s.is_empty()).map(OsString::from),
        ),
    ];
    for (source, value) in candidates {
        let Some(value) = value else {
            tried.push(format!("{source} (not set)"));
            continue;
        };
        if let Some(found) = find_program(&value, path.as_deref()) {
            let mut cmd = Command::new(found);
            if !argv.is_empty() {
                cmd.arg("-e").args(argv);
            }
            cmd.current_dir(dir);
            return Ok(cmd);
        }
        tried.push(format!("{source} {value:?} (not an executable)"));
    }
    Err(AppsError::NoTerminal { tried })
}

/// Opens a terminal in `dir`, detached. The terminal's later exit is not
/// reported.
pub fn open_terminal(dir: &Path, setting: Option<&str>) -> Result<(), AppsError> {
    let cmd = terminal_command(dir, setting)?;
    spawn_detached(cmd, "terminal")
}

/// `name` as a path when it has a separator, else the first executable of
/// that name in `path`.
fn find_program(name: &OsStr, path: Option<&OsStr>) -> Option<PathBuf> {
    if name.as_encoded_bytes().contains(&b'/') {
        let p = PathBuf::from(name);
        return is_executable(&p).then_some(p);
    }
    std::env::split_paths(path?)
        .filter(|d| !d.as_os_str().is_empty())
        .map(|d| d.join(name))
        .find(|p| is_executable(p))
}

fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}
