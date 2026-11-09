// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Choosing a terminal emulator from the user's and the system's lists of
//! preferred terminals.
//!
//! Specification: the Default Terminal Execution Specification, as proposed
//! at gitlab.freedesktop.org/terminal-wg/specifications (merge request 3),
//! read from its published text. Sections used:
//! - "Desktop entry for a terminal": an eligible entry has the
//!   `TerminalEmulator` category; `X-TerminalArgExec=` holds the argument
//!   placed before a command, and an explicitly empty value omits it.
//! - "Additional argument keys": `X-TerminalArgDir=` sets the working
//!   directory; a key ending in `=` takes its value in the same argument.
//! - "Configuration", "Location" and "File format": `xdg-terminals.list`
//!   and `${desktop}-xdg-terminals.list` in the config hierarchy, then
//!   `xdg-terminal-exec/` in the system data directories; one entry ID per
//!   line, optionally `:action`, with `-` and `+` prefixes.
//! - "Priority of selecting an entry": listed entries first, then every
//!   installed terminal entry.
//! - "Execution argument and Execution": the terminal's own `Exec`, its
//!   directory argument, the execution argument, then the command.
//!
//! The older `ExecArg` and `X-ExecArg` keys are read when `X-TerminalArgExec`
//! is absent, and a terminal with none of them gets `-e`: most terminals ship
//! no `X-TerminalArgExec` yet, and `-e` is what they accept.
//!
//! The specification leaves two things open, settled here:
//! - The order of entries under one data directory is undefined. Entries are
//!   tried in the order of their desktop file IDs, directories in priority
//!   order, so the same machine always picks the same terminal.
//! - `+terminal.desktop` is read as: the ID counts as already seen, so a
//!   later `-terminal.desktop` in a lower-priority list cannot exclude it. It
//!   does not select the entry.
//!
//! Directives (lines without `.desktop`) are discarded, as the specification
//! says for those it does not list.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use kernkraft_fs::BaseDirs;

use crate::desktop_entry::DesktopEntry;
use crate::desktop_entry::{read_groups, split_list, unescape, walk_applications, Group, Walked};
use crate::error::AppsError;
use crate::exec::{expand_exec, Targets};
use crate::mimeapps::program_exists;

const LIST_NAME: &str = "xdg-terminals.list";
/// Subdirectory of each system data directory that holds the distribution's
/// lists ("Location").
const DATA_LIST_DIR: &str = "xdg-terminal-exec";
const MAIN_GROUP: &str = "Desktop Entry";
const ACTION_GROUP_PREFIX: &str = "Desktop Action ";
const TERMINAL_CATEGORY: &str = "TerminalEmulator";
/// The execution argument of a terminal that names none (compatibility with
/// terminals that predate `X-TerminalArgExec`).
const DEFAULT_EXEC_ARG: &str = "-e";
/// Keys read for the execution argument, in order ("Desktop entry for a
/// terminal"; the last two are the older names).
const EXEC_ARG_KEYS: [&str; 3] = ["X-TerminalArgExec", "X-ExecArg", "ExecArg"];
const DIR_ARG_KEY: &str = "X-TerminalArgDir";

/// A terminal chosen from the lists, ready to run.
#[derive(Debug)]
pub(crate) struct Terminal {
    /// The terminal's own `Exec`, expanded: the program first.
    pub argv: Vec<OsString>,
    /// The argument placed before a command; `None` when the entry says to
    /// omit it.
    pub exec_arg: Option<OsString>,
    /// The `X-TerminalArgDir` key: how this terminal is told its working
    /// directory.
    pub dir_arg: Option<String>,
}

/// What a selection step found.
pub(crate) enum Outcome {
    Found(Terminal),
    /// Nothing usable, with the reasons worth showing.
    Nothing(Vec<String>),
}

/// One line of a list that selects an entry.
struct Listed {
    token: String,
    id: String,
    action: Option<String>,
}

struct AppFile {
    id: String,
    path: PathBuf,
}

/// The lists and installed entries of one machine, read as far as the steps
/// so far needed.
pub(crate) struct Selection<'a> {
    base: &'a BaseDirs,
    path: Option<OsString>,
    explicit: Vec<Listed>,
    /// IDs that fallback must not use: excluded by a list, or listed and
    /// found unusable.
    skipped_ids: HashSet<String>,
    files: Option<Vec<AppFile>>,
}

impl<'a> Selection<'a> {
    /// Reads the lists. A missing list is normal; one that exists and cannot
    /// be read is an error.
    pub fn read(base: &'a BaseDirs, path: Option<OsString>) -> Result<Selection<'a>, AppsError> {
        let mut explicit = Vec::new();
        let mut skipped_ids = HashSet::new();
        let mut seen = HashSet::new();
        for file in list_files(base) {
            let text = match fs::read_to_string(&file) {
                Ok(t) => t,
                Err(e) if e.kind() == ErrorKind::NotFound => continue,
                Err(e) => return Err(AppsError::io(&file, e)),
            };
            for line in text.lines() {
                read_line(line.trim(), &mut seen, &mut explicit, &mut skipped_ids);
            }
        }
        Ok(Selection {
            base,
            path,
            explicit,
            skipped_ids,
            files: None,
        })
    }

    /// The first listed entry that is an installed, usable terminal
    /// ("Priority of selecting an entry", first part).
    pub fn listed(&mut self) -> Result<Outcome, AppsError> {
        if self.explicit.is_empty() {
            return Ok(Outcome::Nothing(vec!["no list names a terminal".into()]));
        }
        let mut notes = Vec::new();
        for at in 0..self.explicit.len() {
            let (token, id, action) = {
                let l = &self.explicit[at];
                (l.token.clone(), l.id.clone(), l.action.clone())
            };
            let file = self
                .files()?
                .iter()
                .find(|f| f.id == id)
                .map(|f| f.path.clone());
            let Some(file) = file else {
                notes.push(format!("{token}: not installed"));
                self.skipped_ids.insert(id);
                continue;
            };
            match load(&file, &id, action.as_deref(), None, self.path.as_deref()) {
                Ok(terminal) => return Ok(Outcome::Found(terminal)),
                Err(reason) => {
                    notes.push(format!("{token}: {reason}"));
                    self.skipped_ids.insert(id);
                }
            }
        }
        Ok(Outcome::Nothing(notes))
    }

    /// The first installed terminal entry that no list excluded
    /// ("Priority of selecting an entry", second part): checked in the order
    /// of the directories, then of the desktop file IDs, and including the
    /// `OnlyShowIn` and `NotShowIn` conditions.
    pub fn fallback(&mut self) -> Result<Outcome, AppsError> {
        let desktops = self.base.current_desktop.clone();
        let skipped = self.skipped_ids.clone();
        let candidates: Vec<(String, PathBuf)> = self
            .files()?
            .iter()
            .filter(|f| !skipped.contains(&f.id))
            .map(|f| (f.id.clone(), f.path.clone()))
            .collect();
        for (id, file) in candidates {
            if let Ok(terminal) = load(&file, &id, None, Some(&desktops), self.path.as_deref()) {
                return Ok(Outcome::Found(terminal));
            }
        }
        Ok(Outcome::Nothing(vec![
            "no terminal entry is installed".into()
        ]))
    }

    /// Every `.desktop` file under the `applications` directories, the
    /// highest-priority directory first and by ID within one directory; an
    /// ID found in a higher-priority directory hides the same ID below it.
    fn files(&mut self) -> Result<&[AppFile], AppsError> {
        if self.files.is_none() {
            let mut dirs = vec![self.base.data.join("applications")];
            dirs.extend(self.base.data_dirs.iter().map(|d| d.join("applications")));
            let mut all: Vec<AppFile> = Vec::new();
            let mut seen = HashSet::new();
            for dir in &dirs {
                let mut here: Vec<AppFile> = Vec::new();
                walk_applications(dir, dir, &mut HashSet::new(), &mut |walked| {
                    // A path that cannot be used is not a terminal either.
                    if let Walked::File { id, path } = walked {
                        here.push(AppFile { id, path });
                    }
                })?;
                here.sort_by(|a, b| a.id.cmp(&b.id));
                all.extend(here.into_iter().filter(|f| seen.insert(f.id.clone())));
            }
            self.files = Some(all);
        }
        Ok(self.files.as_deref().unwrap_or_default())
    }
}

/// The list files in decreasing priority ("Location").
fn list_files(base: &BaseDirs) -> Vec<PathBuf> {
    let desktops: Vec<String> = base
        .current_desktop
        .iter()
        .map(|d| d.to_lowercase())
        .collect();
    let names: Vec<String> = desktops
        .iter()
        .map(|d| format!("{d}-{LIST_NAME}"))
        .chain(std::iter::once(LIST_NAME.to_string()))
        .collect();
    let mut dirs = vec![base.config.clone()];
    dirs.extend(base.config_dirs.iter().cloned());
    dirs.extend(base.data_dirs.iter().map(|d| d.join(DATA_LIST_DIR)));
    dirs.iter()
        .flat_map(|dir| names.iter().map(move |n| dir.join(n)))
        .collect()
}

/// One line of a list ("File format"). `seen` holds the entries already
/// decided by a higher-priority line, whatever their prefix.
fn read_line(
    line: &str,
    seen: &mut HashSet<String>,
    explicit: &mut Vec<Listed>,
    skipped_ids: &mut HashSet<String>,
) {
    if line.is_empty() || line.starts_with('#') {
        return;
    }
    let (prefix, token) = match line.chars().next() {
        Some(c @ ('-' | '+')) => (Some(c), line[1..].trim()),
        _ => (None, line),
    };
    if !token.contains(".desktop") || !seen.insert(token.to_string()) {
        return;
    }
    let (id, action) = match token.split_once(':') {
        Some((id, action)) => (id, Some(action.to_string())),
        None => (token, None),
    };
    match prefix {
        None => explicit.push(Listed {
            token: token.to_string(),
            id: id.to_string(),
            action,
        }),
        Some('-') => {
            skipped_ids.insert(id.to_string());
        }
        // `+`: already counted as seen above.
        Some(_) => {}
    }
}

/// Reads the entry at `file` and checks it for use as a terminal, with
/// `action` as the action to run. `show_in` is the current desktop names
/// when `OnlyShowIn` and `NotShowIn` are to be applied. The error is the
/// reason the entry is not usable.
fn load(
    file: &Path,
    id: &str,
    action: Option<&str>,
    show_in: Option<&[String]>,
    path: Option<&OsStr>,
) -> Result<Terminal, String> {
    let text = fs::read_to_string(file).map_err(|e| e.to_string())?;
    let action_group = action.map(|a| format!("{ACTION_GROUP_PREFIX}{a}"));
    let groups = read_groups(file, &text, &|name| {
        name == MAIN_GROUP || action_group.as_deref() == Some(name)
    })
    .map_err(|e| e.to_string())?;
    let main = groups
        .iter()
        .find(|g| g.name == MAIN_GROUP)
        .ok_or_else(|| format!("no [{MAIN_GROUP}] group"))?;

    if main.get("Hidden") == Some("true") {
        return Err("hidden".into());
    }
    if main.get("Type") != Some("Application") {
        return Err("not an application".into());
    }
    let has_category = main
        .get("Categories")
        .map(split_list)
        .is_some_and(|c| c.iter().any(|c| c == TERMINAL_CATEGORY));
    if !has_category {
        return Err(format!("no {TERMINAL_CATEGORY} category"));
    }
    if let Some(try_exec) = main.get("TryExec").map(unescape).filter(|t| !t.is_empty()) {
        if !program_exists(&try_exec, path) {
            return Err(format!("TryExec {try_exec:?} is not installed"));
        }
    }
    if let Some(desktops) = show_in {
        if !shown_in(main, desktops) {
            return Err("not shown in this desktop".into());
        }
    }

    let exec = match (&action_group, action) {
        (Some(group), Some(name)) => groups
            .iter()
            .find(|g| g.name == group)
            .and_then(|g| g.get("Exec"))
            .ok_or_else(|| format!("no action {name:?}"))?,
        _ => main.get("Exec").ok_or("no Exec")?,
    };
    let entry = DesktopEntry {
        id: id.to_string(),
        name: id.to_string(),
        exec: unescape(exec),
        icon: None,
        mime_types: Vec::new(),
        terminal: false,
        no_display: false,
        try_exec: None,
        hidden: false,
        path: None,
        source: file.to_path_buf(),
    };
    let mut launches = expand_exec(&entry, Targets::Files(&[])).map_err(|e| e.to_string())?;
    let argv = launches.pop().ok_or("Exec leaves nothing to run")?;

    let exec_arg = EXEC_ARG_KEYS
        .iter()
        .find_map(|k| main.get(k))
        .map(unescape)
        .unwrap_or_else(|| DEFAULT_EXEC_ARG.to_string());
    Ok(Terminal {
        argv,
        exec_arg: (!exec_arg.is_empty()).then(|| OsString::from(exec_arg)),
        dir_arg: main
            .get(DIR_ARG_KEY)
            .map(unescape)
            .filter(|d| !d.is_empty()),
    })
}

/// Desktop Entry Specification 1.5, "Recognized desktop entry keys":
/// `OnlyShowIn` lists the desktops the entry is for, `NotShowIn` those it is
/// not for; a desktop is one of the names in `$XDG_CURRENT_DESKTOP`.
fn shown_in(main: &Group<'_>, desktops: &[String]) -> bool {
    let named = |key: &str| main.get(key).map(split_list).unwrap_or_default();
    let only = named("OnlyShowIn");
    let not = named("NotShowIn");
    let any = |list: &[String]| desktops.iter().any(|d| list.contains(d));
    (only.is_empty() || any(&only)) && !any(&not)
}
