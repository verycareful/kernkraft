// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Finding a terminal emulator and building the command that opens it.
//!
//! Sources, in order, and how each is invoked with a program to run:
//! 1. The caller's setting (Kern's own preference), run as `<setting> -e
//!    <argv>`. It is an explicit choice, so a setting that names no
//!    executable is an error, never a reason to use another terminal.
//! 2. The terminals named in `xdg-terminals.list` files (see
//!    [`crate::terminal_spec`]), run as that terminal's entry says.
//! 3. `$TERMINAL`: a terminal emulator by name or path (the convention of
//!    i3-sensible-terminal and similar launchers), run as `$TERMINAL -e
//!    <argv>`. A value that names no executable is skipped.
//! 4. Any other installed terminal entry, in the order the specification
//!    leaves open and [`crate::terminal_spec`] fixes. It comes after
//!    `$TERMINAL` so that a terminal the user named is never passed over for
//!    one that merely happens to be installed.
//!
//! Each of the setting and `$TERMINAL` is one program, not a command line;
//! `-e` is added only when there is a program to run. The terminal starts in
//! the directory given, through the child's working directory.

use std::ffi::{OsStr, OsString};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use kernkraft_fs::BaseDirs;

use crate::error::AppsError;
use crate::launch::spawn_detached;
use crate::terminal_spec::{Outcome, Selection, Terminal};

/// How a terminal is chosen besides the environment: the user's setting and
/// the XDG directories the lists and entries are read from.
#[derive(Clone, Copy, Debug, Default)]
pub struct TerminalPrefs<'a> {
    /// The user's configured terminal program, the first choice.
    pub setting: Option<&'a str>,
    /// Where the terminal lists and desktop entries are. `None` when there
    /// are none to read (no usable `$HOME`): then only the setting and
    /// `$TERMINAL` can choose.
    pub base: Option<&'a BaseDirs>,
}

/// The command that opens a terminal in `dir`, from the process environment.
/// See the module docs for the order and conventions.
pub fn terminal_command(prefs: &TerminalPrefs<'_>, dir: &Path) -> Result<Command, AppsError> {
    terminal_command_for(&|v| std::env::var_os(v), prefs, dir, &[])
}

/// As [`terminal_command`], reading `PATH` and `TERMINAL` through `env` and
/// running `argv` (program first) in the terminal; an empty `argv` opens a
/// shell. The command's working directory is `dir`.
pub fn terminal_command_for(
    env: &dyn Fn(&str) -> Option<OsString>,
    prefs: &TerminalPrefs<'_>,
    dir: &Path,
    argv: &[OsString],
) -> Result<Command, AppsError> {
    let path = env("PATH");
    let mut tried = Vec::new();

    match prefs.setting.filter(|s| !s.is_empty()) {
        Some(name) => {
            let name = OsString::from(name);
            return match find_program(&name, path.as_deref()) {
                Some(found) => Ok(dash_e_command(found, dir, argv)),
                None => Err(AppsError::NoTerminal {
                    tried: vec![format!("setting {name:?} (not an executable)")],
                }),
            };
        }
        None => tried.push("setting (not set)".to_owned()),
    }

    let mut selection = match prefs.base {
        Some(base) => Some(Selection::read(base, path.clone())?),
        None => {
            tried.push("terminal lists (no usable $HOME)".to_owned());
            None
        }
    };
    if let Some(selection) = &mut selection {
        match selection.listed()? {
            Outcome::Found(terminal) => {
                return Ok(terminal_command_of(&terminal, path.as_deref(), dir, argv))
            }
            Outcome::Nothing(notes) => tried.push(format!("terminal lists ({})", notes.join("; "))),
        }
    }

    match env("TERMINAL").filter(|t| !t.is_empty()) {
        Some(name) => match find_program(&name, path.as_deref()) {
            Some(found) => return Ok(dash_e_command(found, dir, argv)),
            None => tried.push(format!("$TERMINAL {name:?} (not an executable)")),
        },
        None => tried.push("$TERMINAL (not set)".to_owned()),
    }

    if let Some(selection) = &mut selection {
        match selection.fallback()? {
            Outcome::Found(terminal) => {
                return Ok(terminal_command_of(&terminal, path.as_deref(), dir, argv))
            }
            Outcome::Nothing(notes) => {
                tried.push(format!("installed terminals ({})", notes.join("; ")))
            }
        }
    }
    Err(AppsError::NoTerminal { tried })
}

/// `program` run in `dir`, with `-e <argv>` after it when there is a program
/// to run.
fn dash_e_command(program: PathBuf, dir: &Path, argv: &[OsString]) -> Command {
    let mut cmd = Command::new(program);
    if !argv.is_empty() {
        cmd.arg("-e").args(argv);
    }
    cmd.current_dir(dir);
    cmd
}

/// The command for a terminal chosen from an entry: its own `Exec`, the
/// directory argument when the entry has one, then the execution argument
/// and `argv` when there is a program to run ("Execution argument and
/// Execution").
fn terminal_command_of(
    terminal: &Terminal,
    path: Option<&OsStr>,
    dir: &Path,
    argv: &[OsString],
) -> Command {
    let program = &terminal.argv[0];
    let program = find_program(program, path).unwrap_or_else(|| PathBuf::from(program));
    let mut cmd = Command::new(program);
    cmd.args(&terminal.argv[1..]);
    if let Some(key) = &terminal.dir_arg {
        // A key ending in `=` takes its value in the same argument
        // ("Additional argument keys").
        if key.ends_with('=') {
            let mut joined = OsString::from(key);
            joined.push(dir);
            cmd.arg(joined);
        } else {
            cmd.arg(key).arg(dir);
        }
    }
    if !argv.is_empty() {
        if let Some(exec_arg) = &terminal.exec_arg {
            cmd.arg(exec_arg);
        }
        cmd.args(argv);
    }
    cmd.current_dir(dir);
    cmd
}

/// Opens a terminal in `dir`, detached. The terminal's later exit is not
/// reported.
pub fn open_terminal(prefs: &TerminalPrefs<'_>, dir: &Path) -> Result<(), AppsError> {
    let cmd = terminal_command(prefs, dir)?;
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
