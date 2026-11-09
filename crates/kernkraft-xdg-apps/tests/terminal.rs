// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Terminal discovery. Each test builds a machine in a temp directory: a
//! home, the XDG config and data directories, and a `PATH` of stub scripts.
//! Nothing outside it is read, and no real terminal is started.
//!
//! Expectations come from the Default Terminal Execution Specification
//! (sections "Desktop entry for a terminal", "Additional argument keys",
//! "Configuration", "Priority of selecting an entry" and "Execution
//! argument and Execution") and from the order documented in `terminal.rs`.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use kernkraft_fs::base_dirs_from;
use kernkraft_xdg_apps::{terminal_command_for, AppsError, TerminalPrefs};

struct Machine {
    _root: tempfile::TempDir,
    root: PathBuf,
}

impl Machine {
    fn new() -> Machine {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let m = Machine { _root: dir, root };
        for stub in ["env-term", "set-term", "tryexec-ok"] {
            let p = m.root.join("bin").join(stub);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, "#!/bin/sh\nexit 0\n").unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        }
        m
    }

    /// Writes `text` at `rel` below the machine root, making directories.
    fn write(&self, rel: &str, text: &str) {
        let p = self.root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    /// An entry in the user's applications directory.
    fn user_app(&self, name: &str, text: &str) {
        self.write(&format!("home/.local/share/applications/{name}"), text);
    }

    /// An entry in the system applications directory.
    fn system_app(&self, name: &str, text: &str) {
        self.write(&format!("sys/share/applications/{name}"), text);
    }

    fn env(&self, desktop: Option<&str>, terminal: Option<&str>) -> HashMap<String, OsString> {
        let r = &self.root;
        let mut vars: HashMap<String, OsString> = HashMap::new();
        vars.insert("PATH".into(), r.join("bin").into());
        vars.insert("HOME".into(), r.join("home").into());
        vars.insert("XDG_CONFIG_HOME".into(), r.join("home/.config").into());
        vars.insert("XDG_DATA_HOME".into(), r.join("home/.local/share").into());
        vars.insert("XDG_CONFIG_DIRS".into(), r.join("etc").into());
        vars.insert("XDG_DATA_DIRS".into(), r.join("sys/share").into());
        if let Some(d) = desktop {
            vars.insert("XDG_CURRENT_DESKTOP".into(), d.into());
        }
        if let Some(t) = terminal {
            vars.insert("TERMINAL".into(), t.into());
        }
        vars
    }

    fn run_with(
        &self,
        desktop: Option<&str>,
        terminal: Option<&str>,
        setting: Option<&str>,
        argv: &[&str],
    ) -> Result<Command, AppsError> {
        let vars = self.env(desktop, terminal);
        let base = base_dirs_from(&|k| vars.get(k).cloned()).unwrap();
        let argv: Vec<OsString> = argv.iter().map(OsString::from).collect();
        terminal_command_for(
            &|k| vars.get(k).cloned(),
            &TerminalPrefs {
                setting,
                base: Some(&base),
            },
            &self.root,
            &argv,
        )
    }

    fn run(&self, argv: &[&str]) -> Result<Command, AppsError> {
        self.run_with(None, None, None, argv)
    }
}

/// A terminal entry whose program is `exec`, with `extra` lines after it.
fn term(exec: &str, extra: &str) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=T\nExec={exec}\nCategories=System;TerminalEmulator;\n{extra}"
    )
}

/// The program (by file name) and arguments of `cmd`.
fn line(cmd: &Command) -> Vec<String> {
    let name = |s: &std::ffi::OsStr| s.to_string_lossy().into_owned();
    let program = name(cmd.get_program());
    let program = Path::new(&program)
        .file_name()
        .map_or(program.clone(), name);
    std::iter::once(program)
        .chain(cmd.get_args().map(name))
        .collect()
}

fn tried(err: AppsError) -> Vec<String> {
    let AppsError::NoTerminal { tried } = err else {
        panic!("{err:?}")
    };
    tried
}

#[test]
fn nothing_installed_names_every_source() {
    let m = Machine::new();
    let t = tried(m.run(&[]).unwrap_err());
    assert_eq!(t.len(), 4, "{t:?}");
    assert!(t[0].contains("setting"));
    assert!(t[1].contains("terminal lists"));
    assert!(t[2].contains("$TERMINAL"));
    assert!(t[3].contains("installed terminals"));
}

#[test]
fn setting_then_lists_then_env_then_installed() {
    let m = Machine::new();
    m.user_app("a.desktop", &term("a-term", ""));
    m.user_app("b.desktop", &term("b-term", ""));
    let argv = ["htop", "-d"];

    // Only installed terminals: the first by ID.
    assert_eq!(line(&m.run(&argv).unwrap()), ["a-term", "-e", "htop", "-d"]);

    // $TERMINAL beats an installed terminal that no list names.
    let cmd = m.run_with(None, Some("env-term"), None, &argv).unwrap();
    assert_eq!(line(&cmd), ["env-term", "-e", "htop", "-d"]);

    // A list beats $TERMINAL.
    m.write("home/.config/xdg-terminals.list", "b.desktop\n");
    let cmd = m.run_with(None, Some("env-term"), None, &argv).unwrap();
    assert_eq!(line(&cmd), ["b-term", "-e", "htop", "-d"]);

    // The setting beats the list.
    let cmd = m
        .run_with(None, Some("env-term"), Some("set-term"), &argv)
        .unwrap();
    assert_eq!(line(&cmd), ["set-term", "-e", "htop", "-d"]);
}

#[test]
fn the_terminal_starts_in_the_given_directory_and_a_shell_has_no_execution_argument() {
    let m = Machine::new();
    m.user_app("a.desktop", &term("a-term", ""));
    let cmd = m.run(&[]).unwrap();
    assert_eq!(cmd.get_current_dir(), Some(m.root.as_path()));
    assert_eq!(line(&cmd), ["a-term"]);
}

#[test]
fn a_setting_naming_no_executable_is_an_error_not_a_fallback() {
    let m = Machine::new();
    m.user_app("a.desktop", &term("a-term", ""));
    let err = m
        .run_with(None, Some("env-term"), Some("no-such-terminal"), &[])
        .unwrap_err();
    let t = tried(err);
    assert_eq!(t.len(), 1, "{t:?}");
    assert!(t[0].contains("no-such-terminal") && t[0].contains("not an executable"));
}

#[test]
fn an_empty_setting_counts_as_not_set() {
    let m = Machine::new();
    m.user_app("a.desktop", &term("a-term", ""));
    let cmd = m.run_with(None, None, Some(""), &[]).unwrap();
    assert_eq!(line(&cmd), ["a-term"]);
}

#[test]
fn a_terminal_variable_naming_no_executable_is_skipped_and_named() {
    let m = Machine::new();
    let err = m
        .run_with(None, Some("no-such-terminal"), None, &[])
        .unwrap_err();
    let t = tried(err);
    assert!(t[2].contains("no-such-terminal") && t[2].contains("not an executable"));
}

#[test]
fn list_files_are_read_in_decreasing_priority() {
    let m = Machine::new();
    for n in ["a", "b", "c", "d", "e"] {
        m.user_app(&format!("{n}.desktop"), &term(&format!("{n}-term"), ""));
    }
    let pick =
        |m: &Machine| line(&m.run_with(Some("Sway:Wlroots"), None, None, &[]).unwrap())[0].clone();

    // Lowest priority first: the data directory's list, then the config
    // directory's, then the user's, each beating the one before.
    m.write(
        "sys/share/xdg-terminal-exec/xdg-terminals.list",
        "e.desktop\n",
    );
    assert_eq!(pick(&m), "e-term");
    m.write("etc/xdg-terminals.list", "d.desktop\n");
    assert_eq!(pick(&m), "d-term");
    m.write("home/.config/xdg-terminals.list", "c.desktop\n");
    assert_eq!(pick(&m), "c-term");

    // In one directory the desktop-specific list beats the general one, and
    // the desktops come in the order of $XDG_CURRENT_DESKTOP.
    m.write("home/.config/wlroots-xdg-terminals.list", "b.desktop\n");
    assert_eq!(pick(&m), "b-term");
    m.write("home/.config/sway-xdg-terminals.list", "a.desktop\n");
    assert_eq!(pick(&m), "a-term");
}

#[test]
fn a_list_for_another_desktop_is_not_read() {
    let m = Machine::new();
    m.user_app("a.desktop", &term("a-term", ""));
    m.user_app("b.desktop", &term("b-term", ""));
    m.write("home/.config/kde-xdg-terminals.list", "b.desktop\n");
    let cmd = m.run_with(Some("Sway"), None, None, &[]).unwrap();
    assert_eq!(line(&cmd), ["a-term"]);
    let cmd = m.run_with(Some("KDE"), None, None, &[]).unwrap();
    assert_eq!(line(&cmd), ["b-term"]);
}

#[test]
fn list_syntax_comments_blanks_whitespace_and_directives() {
    let m = Machine::new();
    m.user_app("a.desktop", &term("a-term", ""));
    m.user_app("b.desktop", &term("b-term", ""));
    m.write(
        "home/.config/xdg-terminals.list",
        "# a comment\n\n   /enable_cache   \n\t b.desktop \t\n",
    );
    assert_eq!(line(&m.run(&[]).unwrap()), ["b-term"]);
}

#[test]
fn the_first_listed_usable_entry_wins_and_unusable_ones_are_passed_over() {
    let m = Machine::new();
    m.user_app(
        "plain.desktop",
        "[Desktop Entry]\nType=Application\nName=P\nExec=plain\n",
    );
    m.user_app("hidden.desktop", &term("hidden-term", "Hidden=true\n"));
    m.user_app(
        "link.desktop",
        "[Desktop Entry]\nType=Link\nName=L\nURL=x\nCategories=TerminalEmulator;\n",
    );
    m.user_app(
        "away.desktop",
        &term("away-term", "TryExec=no-such-program\n"),
    );
    m.user_app("good.desktop", &term("good-term", ""));
    m.write(
        "home/.config/xdg-terminals.list",
        "missing.desktop\nplain.desktop\nhidden.desktop\nlink.desktop\naway.desktop\ngood.desktop\n",
    );
    assert_eq!(line(&m.run(&[]).unwrap()), ["good-term"]);
}

#[test]
fn tryexec_that_is_installed_counts() {
    let m = Machine::new();
    m.user_app("a.desktop", &term("a-term", "TryExec=tryexec-ok\n"));
    assert_eq!(line(&m.run(&[]).unwrap()), ["a-term"]);
}

#[test]
fn the_notes_say_why_each_listed_entry_was_passed_over() {
    let m = Machine::new();
    m.user_app(
        "plain.desktop",
        "[Desktop Entry]\nType=Application\nName=P\nExec=plain\n",
    );
    m.write(
        "home/.config/xdg-terminals.list",
        "missing.desktop\nplain.desktop\n",
    );
    let t = tried(m.run(&[]).unwrap_err());
    assert!(t[1].contains("missing.desktop: not installed"), "{t:?}");
    assert!(
        t[1].contains("plain.desktop: no TerminalEmulator category"),
        "{t:?}"
    );
}

#[test]
fn a_listed_action_runs_the_actions_exec() {
    let m = Machine::new();
    m.user_app(
        "k.desktop",
        &term(
            "k-term",
            "\n[Desktop Action new-window]\nName=New\nExec=k-term --new-window\n",
        ),
    );
    m.user_app("b.desktop", &term("b-term", ""));
    m.write("home/.config/xdg-terminals.list", "k.desktop:new-window\n");
    assert_eq!(line(&m.run(&[]).unwrap()), ["k-term", "--new-window"]);

    // An action the entry does not have makes the line unusable.
    m.write(
        "home/.config/xdg-terminals.list",
        "k.desktop:nope\nb.desktop\n",
    );
    assert_eq!(line(&m.run(&[]).unwrap()), ["b-term"]);
}

#[test]
fn the_entrys_own_arguments_come_before_the_command() {
    let m = Machine::new();
    m.user_app("a.desktop", &term("a-term --class kern", ""));
    assert_eq!(
        line(&m.run(&["htop"]).unwrap()),
        ["a-term", "--class", "kern", "-e", "htop"]
    );
}

#[test]
fn the_execution_argument_comes_from_the_entry() {
    let m = Machine::new();
    let argv = ["htop"];
    let cases = [
        // The key of the specification.
        ("X-TerminalArgExec=--\n", vec!["a-term", "--", "htop"]),
        // Explicitly empty: omitted.
        ("X-TerminalArgExec=\n", vec!["a-term", "htop"]),
        // No key: the default for terminals that predate it.
        ("", vec!["a-term", "-e", "htop"]),
        // The older names.
        ("ExecArg=-x\n", vec!["a-term", "-x", "htop"]),
        ("X-ExecArg=-y\n", vec!["a-term", "-y", "htop"]),
        // The specification's key wins over the older one.
        (
            "X-TerminalArgExec=-z\nExecArg=-x\n",
            vec!["a-term", "-z", "htop"],
        ),
    ];
    for (extra, expected) in cases {
        m.user_app("a.desktop", &term("a-term", extra));
        assert_eq!(line(&m.run(&argv).unwrap()), expected, "{extra:?}");
    }
}

#[test]
fn the_directory_argument_follows_the_entrys_key() {
    let m = Machine::new();
    let dir = m.root.to_string_lossy().into_owned();

    // A key ending in `=` takes its value in the same argument.
    m.user_app(
        "a.desktop",
        &term("a-term", "X-TerminalArgDir=--working-directory=\n"),
    );
    assert_eq!(
        line(&m.run(&["htop"]).unwrap()),
        [
            "a-term".to_string(),
            format!("--working-directory={dir}"),
            "-e".into(),
            "htop".into()
        ]
    );

    // Any other key takes it as the next argument.
    m.user_app("a.desktop", &term("a-term", "X-TerminalArgDir=--cwd\n"));
    assert_eq!(
        line(&m.run(&[]).unwrap()),
        ["a-term".to_string(), "--cwd".into(), dir]
    );
}

#[test]
fn installed_terminals_are_tried_by_id_and_the_user_directory_first() {
    let m = Machine::new();
    m.system_app("a.desktop", &term("sys-a", ""));
    m.user_app("b.desktop", &term("user-b", ""));
    // The user's directory is tried first, though its ID sorts later.
    assert_eq!(line(&m.run(&[]).unwrap()), ["user-b"]);
    // Within a directory, by ID.
    m.user_app("a2.desktop", &term("user-a2", ""));
    assert_eq!(line(&m.run(&[]).unwrap()), ["user-a2"]);
}

#[test]
fn an_entry_hides_the_same_id_in_a_lower_priority_directory() {
    let m = Machine::new();
    m.system_app("a.desktop", &term("sys-a", ""));
    m.user_app("a.desktop", &term("user-a", ""));
    assert_eq!(line(&m.run(&[]).unwrap()), ["user-a"]);

    // A hidden copy deletes the ID.
    m.user_app("a.desktop", &term("user-a", "Hidden=true\n"));
    assert!(m.run(&[]).is_err());
}

#[test]
fn ids_of_entries_in_subdirectories_use_a_dash() {
    let m = Machine::new();
    m.write(
        "home/.local/share/applications/sub/t.desktop",
        &term("sub-term", ""),
    );
    m.write("home/.config/xdg-terminals.list", "sub-t.desktop\n");
    assert_eq!(line(&m.run(&[]).unwrap()), ["sub-term"]);
}

#[test]
fn fallback_respects_exclusions_and_protection() {
    let m = Machine::new();
    m.user_app("a.desktop", &term("a-term", ""));
    m.user_app("b.desktop", &term("b-term", ""));

    // `-` excludes an entry from fallback.
    m.write("home/.config/xdg-terminals.list", "-a.desktop\n");
    assert_eq!(line(&m.run(&[]).unwrap()), ["b-term"]);

    // A lower-priority `-` cannot exclude an entry a higher list protects
    // with `+`.
    m.write("home/.config/xdg-terminals.list", "+a.desktop\n");
    m.write("etc/xdg-terminals.list", "-a.desktop\n");
    assert_eq!(line(&m.run(&[]).unwrap()), ["a-term"]);

    // Without the protection the exclusion applies.
    m.write("home/.config/xdg-terminals.list", "# nothing\n");
    assert_eq!(line(&m.run(&[]).unwrap()), ["b-term"]);

    // A higher-priority `-` also stops a lower-priority list from selecting.
    m.write("home/.config/xdg-terminals.list", "-b.desktop\n");
    m.write("etc/xdg-terminals.list", "b.desktop\n");
    assert_eq!(line(&m.run(&[]).unwrap()), ["a-term"]);
}

#[test]
fn show_in_conditions_apply_to_fallback_only() {
    let m = Machine::new();
    m.user_app("a-kde.desktop", &term("kde-term", "OnlyShowIn=KDE;\n"));
    m.user_app(
        "b-not-sway.desktop",
        &term("notsway-term", "NotShowIn=Sway;\n"),
    );
    m.user_app(
        "c-sway.desktop",
        &term("sway-term", "OnlyShowIn=GNOME;Sway;\n"),
    );

    // Fallback under Sway skips the first two.
    let cmd = m.run_with(Some("Sway"), None, None, &[]).unwrap();
    assert_eq!(line(&cmd), ["sway-term"]);

    // A listed entry is used whatever its conditions.
    m.write("home/.config/xdg-terminals.list", "a-kde.desktop\n");
    let cmd = m.run_with(Some("Sway"), None, None, &[]).unwrap();
    assert_eq!(line(&cmd), ["kde-term"]);
}

#[test]
fn without_a_usable_home_only_the_setting_and_the_variable_choose() {
    let m = Machine::new();
    m.user_app("a.desktop", &term("a-term", ""));
    let none = TerminalPrefs {
        setting: None,
        base: None,
    };

    let vars = m.env(None, Some("env-term"));
    let cmd = terminal_command_for(&|k| vars.get(k).cloned(), &none, &m.root, &[]).unwrap();
    assert_eq!(line(&cmd), ["env-term"]);

    let vars = m.env(None, None);
    let err = terminal_command_for(&|k| vars.get(k).cloned(), &none, &m.root, &[]).unwrap_err();
    let t = tried(err);
    assert!(t.iter().any(|n| n.contains("no usable $HOME")), "{t:?}");
}

#[test]
fn a_list_that_cannot_be_read_is_an_error() {
    let m = Machine::new();
    // A directory where the list file should be: reading it fails with
    // something other than "not found".
    fs::create_dir_all(m.root.join("home/.config/xdg-terminals.list")).unwrap();
    let err = m.run(&[]).unwrap_err();
    assert!(matches!(err, AppsError::Io { .. }), "{err:?}");
}
