// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Starting applications and setting default applications.
//!
//! Specifications: Desktop Entry Specification 1.5, "The Exec key" (what to
//! run, see [`crate::expand_exec`]) and "Recognized desktop entry keys"
//! (`Path`, `Terminal`); MIME Applications Associations 1.0.1, section 4
//! (setting a default is delegated to `xdg-mime default`, which owns the
//! write to the user's `mimeapps.list`).

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::desktop_entry::DesktopEntry;
use crate::error::AppsError;
use crate::exec::expand_exec;
use crate::terminal::terminal_command_for;

/// Starts `entry` for `files`, detached from this process.
///
/// One program per [`expand_exec`] argument vector. The working directory is
/// the entry's `Path`, else `cwd`, else this process's. With `Terminal=true`
/// the program runs inside a terminal (see [`launch_in_terminal`]).
///
/// The program is not waited for: its later exit status is not reported.
/// Failure to start it is [`AppsError::Launch`]; when several launches are
/// needed, the first failure stops the rest.
pub fn launch(
    entry: &DesktopEntry,
    files: &[PathBuf],
    cwd: Option<&Path>,
) -> Result<(), AppsError> {
    launch_in_terminal(entry, files, cwd, None)
}

/// As [`launch`], with `terminal_setting` as the last choice of terminal for
/// a `Terminal=true` entry (see [`crate::terminal_command`]).
pub fn launch_in_terminal(
    entry: &DesktopEntry,
    files: &[PathBuf],
    cwd: Option<&Path>,
    terminal_setting: Option<&str>,
) -> Result<(), AppsError> {
    let launches = expand_exec(entry, files)?;
    let dir = entry.path.as_deref().or(cwd);
    for argv in launches {
        let cmd = if entry.terminal {
            let here;
            let dir = match dir {
                Some(d) => d,
                None => {
                    here = std::env::current_dir().map_err(|e| AppsError::Launch {
                        id: entry.id.clone(),
                        source: e,
                    })?;
                    &here
                }
            };
            terminal_command_for(&|v| std::env::var_os(v), dir, terminal_setting, &argv)?
        } else {
            let mut cmd = Command::new(&argv[0]);
            cmd.args(&argv[1..]);
            if let Some(d) = dir {
                cmd.current_dir(d);
            }
            cmd
        };
        spawn_detached(cmd, &entry.id)?;
    }
    Ok(())
}

/// Starts `cmd` in its own session with stdin, stdout and stderr on
/// `/dev/null`, so it outlives this process and a closing terminal.
///
/// A thread waits for the child. Without it the exited child stays a zombie
/// for as long as this process runs; a double fork would avoid that too but
/// needs an intermediate process and `fork` in a multithreaded program. The
/// thread only holds the `Child` and ends when the program does. If the
/// thread cannot be created the launched process is killed and reaped and the launch fails.
pub(crate) fn spawn_detached(mut cmd: Command, id: &str) -> Result<(), AppsError> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: the closure runs between fork and exec and calls only
    // `setsid`, a plain system call that neither allocates nor takes locks.
    unsafe {
        cmd.pre_exec(|| {
            rustix::process::setsid()
                .map(|_| ())
                .map_err(std::io::Error::from)
        });
    }
    let child = cmd.spawn().map_err(|source| AppsError::Launch {
        id: id.to_string(),
        source,
    })?;
    let slot = std::sync::Arc::new(std::sync::Mutex::new(Some(child)));
    let theirs = std::sync::Arc::clone(&slot);
    let waiter = std::thread::Builder::new()
        .name("kernkraft-reap".to_string())
        .spawn(move || {
            let child = theirs.lock().ok().and_then(|mut c| c.take());
            if let Some(mut child) = child {
                let _ = child.wait();
            }
        });
    if let Err(source) = waiter {
        // Without a waiter the child would end as a zombie. The child is
        // still in `slot`: kill and reap it here (for `Terminal=true` that is
        // the terminal emulator process), then report it.
        if let Some(mut child) = slot.lock().ok().and_then(|mut c| c.take()) {
            let _ = child.kill();
            let _ = child.wait();
        }
        return Err(AppsError::Launch {
            id: id.to_string(),
            source: std::io::Error::new(
                source.kind(),
                format!(
                    "no thread to reap the program could be created, so the launched process was killed and reaped: {source}"
                ),
            ),
        });
    }
    Ok(())
}

/// The command `set_default` runs: `xdg-mime default <desktop_id> <mime>`.
/// Fails with [`AppsError::InvalidArgument`] for an empty value or one that
/// starts with `-`, which `xdg-mime` would read as an option.
pub fn set_default_command(desktop_id: &str, mime: &str) -> Result<Command, AppsError> {
    for (what, value) in [("desktop file ID", desktop_id), ("MIME type", mime)] {
        if value.is_empty() || value.starts_with('-') {
            return Err(AppsError::InvalidArgument {
                what,
                value: value.to_string(),
            });
        }
    }
    let mut cmd = Command::new("xdg-mime");
    cmd.args(["default", desktop_id, mime]);
    Ok(cmd)
}

/// Makes `desktop_id` the default application for `mime` by running
/// `xdg-mime default`. A non-zero exit is [`AppsError::XdgMime`] with the
/// program's standard error; `xdg-mime` missing is [`AppsError::Launch`].
pub fn set_default(desktop_id: &str, mime: &str) -> Result<(), AppsError> {
    run_xdg_mime(set_default_command(desktop_id, mime)?)
}

fn run_xdg_mime(mut cmd: Command) -> Result<(), AppsError> {
    let output = cmd
        .stdin(Stdio::null())
        .output()
        .map_err(|source| AppsError::Launch {
            id: "xdg-mime".to_string(),
            source,
        })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(AppsError::XdgMime {
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    /// A shell one-liner stands in for `xdg-mime`; no file is written and
    /// then executed, which parallel tests would race on (ETXTBSY).
    fn stub(body: &str) -> Command {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", body]);
        cmd
    }

    #[test]
    fn command_is_xdg_mime_default() {
        let cmd = set_default_command("a.desktop", "text/plain").unwrap();
        assert_eq!(cmd.get_program(), "xdg-mime");
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, ["default", "a.desktop", "text/plain"]);
    }

    #[test]
    fn option_like_values_are_refused() {
        assert!(matches!(
            set_default_command("--help", "text/plain"),
            Err(AppsError::InvalidArgument { .. })
        ));
        assert!(matches!(
            set_default_command("a.desktop", ""),
            Err(AppsError::InvalidArgument { .. })
        ));
    }

    #[test]
    fn nonzero_exit_carries_status_and_stderr() {
        let err = run_xdg_mime(stub("echo boom >&2; exit 3")).unwrap_err();
        match err {
            AppsError::XdgMime { status, stderr } => {
                assert_eq!(status.code(), Some(3));
                assert_eq!(stderr, "boom");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn zero_exit_is_ok() {
        run_xdg_mime(stub("exit 0")).unwrap();
    }
}
