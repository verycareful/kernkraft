# Changelog - kernkraft

All notable changes documented here. Format: [Keep a Changelog](https://keepachangelog.com/), with one deviation: dated release entries are timestamped to the minute with timezone (`YYYY-MM-DD HH:MM (TZ)`).

## [0.1.3.1] - 2026-11-09 23:40 (IST)

A program started from an AppImage no longer inherits the AppImage's environment. Still alpha; no API changes.

### Fixed
- **A terminal, or any other program, started from an AppImage could fail to start**: the AppImage launcher points the environment at the bundle (`PYTHONHOME`, `PYTHONPATH`, `LD_LIBRARY_PATH`, `PERLLIB`, `QT_PLUGIN_PATH`, the GTK, GIO and GStreamer paths, and a bundle entry at the front of `PATH` and `XDG_DATA_DIRS`), and every program started from there inherited it. A program that embeds Python, kitty for one, stopped at start-up with `Failed to import encodings module` because `PYTHONHOME` named a directory with no standard library, while the application reported the launch as done. Programs started by `launch`, `launch_in_terminal`, `launch_groups`, `open_default` and `open_terminal`, and `xdg-mime`, now get the environment without the bundle.

### Changed
- **How the bundle is left**: when `APPDIR` is set, a variable whose entries all lie inside the bundle is removed, and in a list variable (separated by `:`) the entries inside the bundle are dropped and the rest kept in order, so a path of the user's own survives. `APPDIR`, `APPIMAGE`, `ARGV0` and `OWD` are removed. Nothing else is touched, and outside a bundle `APPDIR` is unset and the environment passes through as it is.

### Tests
- `a_started_program_does_not_inherit_the_bundle`: a program started under a bundle environment records the environment it got, with the bundle variables gone, `PATH` and `XDG_DATA_DIRS` shortened to the entries outside the bundle, and an unrelated variable kept.
- Unit tests of the rule: nothing changes outside a bundle; a variable that points only into the bundle is removed, including a path with a doubled `/`; list variables keep their other entries in order; a list left with only empty entries is removed; a directory that only shares the bundle's name as a prefix is not inside it; unrelated variables are left alone; the runtime variables are removed; a non-UTF-8 entry outside the bundle is kept byte for byte.

### Results
- 859 tests across 70 suites, all passed (23.9s, CachyOS x86-64, rustc 1.99.1).

## [0.1.3.0] - 2026-11-09 23:16 (IST)

The terminal a user chooses is now the terminal that opens, and kernkraft reads the terminal lists itself instead of running `xdg-terminal-exec`. Still alpha; the terminal functions of `kernkraft-xdg-apps` change signature, so a direct Rust caller must pass the new `TerminalPrefs`.

### Added
- **The Default Terminal Execution Specification, read natively**: `kernkraft-xdg-apps` reads `xdg-terminals.list` and `${desktop}-xdg-terminals.list` from the config directories, then `xdg-terminal-exec/` under the system data directories, and picks the first listed entry that is an installed terminal (`TerminalEmulator` category, not `Hidden`, `TryExec` present). An entry ID may carry an `:action`, which runs that action's `Exec`; `-` excludes an entry from fallback and `+` protects it from a later exclusion; lines that are not entry IDs are discarded. With no usable list, any installed terminal entry is used, tried in a fixed order (the user's data directory first, then the system ones, by desktop file ID), and `OnlyShowIn` and `NotShowIn` apply to that fallback only.
- **The terminal's own arguments**: the entry's `Exec`, then its `X-TerminalArgDir` (joined to the value when the key ends in `=`), then the execution argument and the command. The execution argument is `X-TerminalArgExec`, else the older `X-ExecArg` or `ExecArg`, else `-e`; an explicitly empty value omits it.
- **`TerminalPrefs`**: the user's terminal setting and the XDG directories to read the lists and entries from.

### Changed
- **The user's terminal setting is now the first choice.** The order is the setting, the terminal lists, `$TERMINAL`, then any other installed terminal. Before, `xdg-terminal-exec` came first and a terminal chosen in Kern's settings was used only when it was absent, so a system with `xdg-terminal-exec` opened its desktop's default terminal whatever the user had chosen.
- **A setting that names no executable is an error**, not a reason to open another terminal. `NoTerminal` names it. A `$TERMINAL` that names no executable is still skipped.
- **`xdg-terminal-exec` is no longer run.** Its job is done by the reader above, so a system without it opens a terminal too.
- **Breaking**: `terminal_command`, `terminal_command_for`, `open_terminal`, `launch_in_terminal`, `launch_groups` and `open_default` take a `TerminalPrefs` where they took the setting as an `Option<&str>`, and the terminal functions of `kernkraft-tauri` read the lists from the directories the plugin was given. Callers that go through the commands are unaffected.
- `NoTerminal` lists every source it tried, in the new order, with the reason each failed.

### Fixed
- **Selecting a terminal in Kern's settings could open another one**: on a system with `xdg-terminal-exec` installed, which picks the desktop's default terminal (Konsole on KDE), the setting was never reached.
- **No terminal opened on a system without `xdg-terminal-exec`** unless `$TERMINAL` or the setting was set, even with a terminal installed.

### Tests
- `setting_then_lists_then_env_then_installed`: each source beats the ones after it.
- `a_setting_naming_no_executable_is_an_error_not_a_fallback`, `an_empty_setting_counts_as_not_set`, `a_terminal_variable_naming_no_executable_is_skipped_and_named`.
- `list_files_are_read_in_decreasing_priority`, `a_list_for_another_desktop_is_not_read`, `list_syntax_comments_blanks_whitespace_and_directives`: list locations, desktop-specific lists in `$XDG_CURRENT_DESKTOP` order, comments, blank lines and directives.
- `the_first_listed_usable_entry_wins_and_unusable_ones_are_passed_over`, `tryexec_that_is_installed_counts`, `the_notes_say_why_each_listed_entry_was_passed_over`, `a_listed_action_runs_the_actions_exec`.
- `the_execution_argument_comes_from_the_entry`, `the_directory_argument_follows_the_entrys_key`, `the_entrys_own_arguments_come_before_the_command`.
- `installed_terminals_are_tried_by_id_and_the_user_directory_first`, `an_entry_hides_the_same_id_in_a_lower_priority_directory`, `ids_of_entries_in_subdirectories_use_a_dash`, `fallback_respects_exclusions_and_protection`, `show_in_conditions_apply_to_fallback_only`.
- `without_a_usable_home_only_the_setting_and_the_variable_choose`, `a_list_that_cannot_be_read_is_an_error`.
- `terminal_apps_use_the_terminal_setting` also shows the setting beating an installed terminal entry.

### Results
- 850 tests across 69 suites, all passed (24.0s, CachyOS x86-64, rustc 1.99.1).

## [0.1.2.1] - 2026-11-09 18:59 (IST)

A `hyprtoolkit.conf` that writes its colours the way Hyprland's own files do is now read. Still alpha; no API changes.

### Fixed
- **`hyprtoolkit.conf` colours in `0xAARRGGBB` and decimal form were ignored**: `Rgba::parse` knew only `rgb(rrggbb)`, `rgba(rrggbbaa)` and `#hex`, so a file written with `0xFF282a36` had every colour line refused and the theme fell through to the portal, GTK and Kern's own tokens. It now also reads `0xAARRGGBB` (alpha first, all eight digits), `rgb(r,g,b)` and `rgba(r,g,b,a)` (decimal channels 0 to 255, alpha a fraction from 0.0 to 1.0). A number out of range, a sign, an exponent, `nan` or `inf` is refused, never clamped. A bare `#rrggbb` in a `.conf` is still a hyprlang comment.

### Tests
- `parses_argb_hyprtoolkit`: a file written in `0xAARRGGBB` fills every colour and both font families.
- `argb_colour_has_alpha_first`: `0x40ff8000` is red 255, green 128, blue 0, alpha 0x40, in either case of `x`.
- `decimal_colour_forms`: `rgb(r,g,b)` and `rgba(r,g,b,a)` with spaces, with alpha 0, .5, 0.933 and 1 rounded to a byte.
- `malformed_colours_are_refused`: 21 malformed inputs (short or long hex, out-of-range, negative, fractional or missing channels, bad alpha) return nothing.
- `every_notation_in_one_file`: five colour notations in one file parse with nothing ignored.

### Results
- 829 tests across 58 suites, all passed (25.0s, CachyOS x86-64, rustc 1.99.1).

## [0.1.2.0] - 2026-11-09 17:58 (IST)

The Documents walk can reach hidden items when asked. Still alpha; the Rust method's signature changes, so a direct Rust caller of `Kernkraft::documents` must pass the new argument.

### Added
- **`documents` takes `hidden`**: the Documents walk of `kernkraft-tauri` lists dotfiles, dot-folders and everything inside them only when `hidden` is set, as the name search already does. The `documents` command accepts it as an optional field, and a caller that sends none gets the walk it always got, without hidden items.

### Changed
- **`Kernkraft::documents(key, roots, hidden, origin)`** takes the new `hidden` argument before `origin`. Breaking for a direct Rust caller; callers that go through the command are unaffected.

### Tests
- `documents_reach_dotfiles_and_dot_folders_only_when_asked`: a dotfile and a file inside a dot-folder are listed only with the flag, and a hidden file that is not text never is.
- `ipc_documents` covers the command with no `hidden` field and with `hidden` true.

### Results
- 824 tests across 58 suites, all passed (24.0s, CachyOS x86-64, rustc 1.99.1).

## [0.1.1.0] - 2026-11-04 23:46 (IST)

The desktop release: seven new crates, among them the Tauri plugin Kern Desktop 0.1.0.0 is built on, and file operations in `kernkraft-fs` that can be undone. Still alpha, and the API changes below break 0.1.0.2 callers.

### Added
- **`kernkraft-text`**: opens UTF-8 text for editing and saves it atomically with its byte order mark and line endings as they were. A file that could not be saved back byte for byte (not UTF-8, UTF-16, mixed line endings, too large, not writable) opens as a read-only copy that says why, and a save never overwrites such a file.
- **`kernkraft-xdg-apps`**: MIME type lookup from the Shared MIME-info Database (globs, aliases, subclasses, text detection), desktop entries, default applications per the MIME Applications Associations spec, launching through `Exec` lines (files or a URL), and terminal discovery.
- **`kernkraft-thumb`**: thumbnails for PNG, JPEG, WebP, GIF and SVG in the freedesktop thumbnail cache that other file managers share, with failure entries so a broken image is not retried on every view.
- **`kernkraft-bus`**: session and system D-Bus connections that resolve the bus address themselves and only ever open a Unix socket.
- **`kernkraft-theme`**: the system theme from Hyprland's `hyprtoolkit.conf`, the portal's appearance settings, GTK and fontconfig, resolved against Kern's design tokens into CSS custom properties, with derived colours checked for WCAG contrast, and a watcher that reports changes live.
- **`kernkraft-volumes`**: mounted volumes from `/proc/self/mountinfo` with udev labels; mount, unmount and eject through UDisks2; a watcher for mounts and devices that come and go. Each volume carries an identity the actions check, so an action never reaches a different disk that took the same place.
- **`kernkraft-tauri`**: the Tauri 2 plugin (`plugin:kernkraft-tauri|<command>`). Paths cross as `WirePath` (non-UTF-8 names survive) and errors as `WireError` (kind, message, details). File manager commands: listing, create, rename, and copy, move, trash and delete as background operations with progress, cancel and conflict prompts; the trash view; undo and redo; folder watching through inotify; properties. Also text open, check and save, a thumbnail queue, the theme, applications and defaults, the terminal, volumes, user folders, a streamed name search and the Documents walk.
- **`kernkraft-fs` undo journal**: every copy, move, trash, rename and create is recorded with stamps of what it left, and undo or redo refuses, naming the file, when something has changed there since.
- **`kernkraft-fs` `same_filesystem`**: whether a move would be a rename, by device number and mount ID (a bind mount shares the device number but not the mount), so a drop moves on one filesystem and copies across.
- **`kernkraft-fs` `measure`**: totals of size and item counts for a properties view, cancellable.
- **`kernkraft-fs` errors**: `MountPoint`, `MountIdUnavailable`, `Changed`, `Irreversible`, `Partial`, `SourceNotRemoved`, `NothingToUndo`, `NothingToRedo`.

### Changed
- **Every save says what it expects at the path** (`Expect`): the file as it was read (`Unchanged`, by stamp), nothing (`Nothing`, so a new file never replaces one that appeared meanwhile), or `Anything`. `write_atomic`, `write_bytes_atomic` and `kernkraft_pdf::save` take it in place of the optional stamp. Breaking.
- A mount point is never moved: a move or merge of one fails with `FsError::MountPoint` before anything is copied or deleted, at any depth of a merge.
- Restoring from the trash recreates a missing original folder; undoing a trash does not, and fails instead.
- In `kernkraft-tauri`, rename and create are refused with `Busy` while a file operation or an undo runs, and an operation or undo is refused while a rename or create runs; search and the Documents walk never conflict with either.

### Results
- 823 tests across 58 suites, all passed (26.1s, CachyOS x86-64, rustc 1.99.1).

## [0.1.0.2] - 2026-10-18 21:55 (IST)

The fixes for the seven known issues 0.1.0.1 pinned. All eleven tests that shipped red now pass.

### Added
- **`FsError::OwnershipNotKept`**: an atomic save that would change a file's owner or group (the file belongs to another user, or to a group this process is not in) is refused, the file untouched.
- **`FsError::XattrsNotKept`**: names the extended attributes a destination would not take.
- **`OpReport::warnings`**: items a copy or move completed without carrying everything over, today only extended attributes the destination refused.

### Changed
- **Loading a PDF checks its page tree.** A cyclic tree, a page reached twice or a tree nested too deeply is now a `PdfError::Parse` at load, not at the first operation. A page-tree node without `/Type` is given the type ISO 32000 requires, and the file's cross-reference and object streams are dropped on load, since every save writes its own.
- `page_count` counts pages with the same walk as every operation. On a document built by hand whose page tree is malformed it is 0, and every operation reports `PdfError::Parse`.

### Fixed
- A page node without `/Type` is numbered the same way by `page_count`, every page operation and the renderer.
- `extract`, every split and `remove` drop the form fields whose widgets were all on dropped pages, together with their appearances. A field with widgets on kept pages keeps those.
- A load and save that edits nothing writes the same bytes again.
- A rendered page is the requested width and the width times the aspect ratio rounded to the nearest pixel, as Kern Android renders it. The page fills that exact size.
- `parse_ranges` takes page numbers in ASCII digits only, so `+1` is a bad range.
- `.trashinfo` files with space around `=` are read.
- Atomic save keeps the owner, group, extended attributes and POSIX ACLs. Copy and move keep the extended attributes of files and directories, reporting any the destination refuses.

### Results
- 260 tests across 16 suites, all passed (0.23s, CachyOS x86-64, rustc 1.99.1).

## [0.1.0.1] - 2026-10-18 21:33 (IST)

The test release for 0.1.0.0: a round-trip corpus, property tests of every page operation, and edge cases for both crates. No library code changed. Eleven of the new tests fail on purpose: each pins a defect that 0.1.0.2 fixes, and they stay red until then.

### Tests
- **PDF corpus**: ten files, built in code so every expectation follows from how each was made: a classic cross-reference table, a cross-reference stream with object streams, Flate-compressed content, an incremental update, a nested page tree, a page whose contents are an array of streams, an indirect `/Kids` array, an image shared by two pages, an `/Info` dictionary, and the sample from `testdata/`.
- **`kernkraft-pdf` round trip** (`roundtrip`): every corpus file loads with its page count and markers; a save and reload keeps every content object, the trailer's `/Root` and `/Info`, and each page's inherited attributes; an atomic save to disk reads back the same.
- **`kernkraft-pdf` properties** (`properties`): over the whole corpus, an identity reorder changes nothing, a reorder and its inverse restore the document, four quarter turns restore every rotation, a split per page or by ranges merges back to the original, extracting every page keeps it whole, and removing any one page leaves the others. Every result holds no reference to a missing object and survives a save and reload.
- **`kernkraft-pdf` edge cases**: page trees that are cyclic, list a page twice, nest 300 deep or have pages without `/Type`; `/Rotate` given by reference, as a real or negative; malformed page ranges; form fields and an open action that point at dropped pages; untitled, foreign and nested-name-tree bookmarks; merging one document twice and inputs with unparsable versions; empty input and bytes after `%%EOF`; every corpus page rendered at the size its boxes and rotation give.
- **`kernkraft-fs` edge cases**: atomic save into a read-only directory, with setgid bits, of 0 bytes and of 3 MiB, under a non-UTF-8 name, and keeping the group, extended attributes and POSIX ACL; trash round trips of non-UTF-8 names and names holding spaces, `%`, newlines and `#`, restoring into a removed folder, restoring onto a path already taken, an info file whose item is gone, and `.trashinfo` parsing (space around `=`, CRLF, repeated keys, a following group); copying a FIFO, onto a dangling symlink, a read-only tree and hard links; cancelling during the second source; progress that never runs backwards or past its total; moving a symlink; extended attributes through copy; duplicate `user-dirs.dirs` keys; scanning duplicate roots, a file root and a FIFO; staging a directory.

### Known issues
Each item below is pinned by a failing test and fixed in 0.1.0.2.
- A page node without `/Type` is a page to the renderer and to every page operation, but `page_count` leaves it out, so page numbers can name different pages.
- `extract`, and every split built on it, keeps the `/AcroForm` fields of dropped pages, and with them those pages' field appearances.
- A load and save that edits nothing rewrites the file: the cross-reference stream moves to a new object number on every save.
- A rendered page is one pixel taller than the width times the aspect ratio, rounded, whenever the exact height has a fraction (MuPDF rounds outward); Kern Android rounds to the nearest pixel, and the height limit check rounds to nearest too.
- `parse_ranges` accepts `+1` as page 1.
- `.trashinfo` files with space around `=` are reported as malformed.
- Atomic save keeps permission bits but drops the group, extended attributes and POSIX ACLs; copy drops extended attributes.

### Results
- 256 tests across 16 suites: 245 passed, 11 failed, every failure a known issue above (0.14s, CachyOS x86-64, rustc 1.99.1).

## [0.1.0.0] - 2026-10-18 13:39 (IST)

The first alpha: the workspace, a complete `kernkraft-fs` and the page operations of `kernkraft-pdf`.

### Added
- **Workspace and gates**: a Cargo workspace whose CI runs rustfmt, clippy with warnings denied, the test suite, a check that no core crate depends on `tauri` or `jni`, and `cargo deny`, which bans every HTTP, socket and async runtime crate. The version label lives in `VERSION` and reaches Cargo as semver build metadata (`0.1.0+0`).
- **`kernkraft-fs` XDG directories**: base directories per the XDG Base Directory Specification (an empty or relative variable is ignored) and user directories from `user-dirs.dirs`, with English defaults for any it leaves out. An unreadable `user-dirs.dirs` is an error, never a silent fallback.
- **`kernkraft-fs` atomic save**: temp file in the target's directory, fsync, permission bits carried over, rename, directory fsync; symlinks are written through and kept. A `FileStamp` (length, mtime, device, inode, ctime) taken at open makes a save refuse when another program wrote, replaced or deleted the file in the meantime. Any failure leaves the target and its directory untouched.
- **`kernkraft-fs` trash**: FreeDesktop Trash 1.0, with the home trash, a mount's shared `.Trash/$uid` and `.Trash-$uid`; list, restore and purge. Items are only ever renamed into a trash, never copied across devices. Listing reports each info file it cannot use. Restore never replaces a file at the original path.
- **`kernkraft-fs` file operations**: create, rename (never replacing anything), permanent delete (never following a symlink), and bulk copy and move with Skip, Replace and Keep Both conflict handling, byte progress and cancel. A copied file is never seen half-written, a folder cannot be placed inside itself, and a move across devices deletes a source only after its copy is complete.
- **`kernkraft-fs` scanning and staging**: sorted directory listing, a depth-limited scan that follows directory symlinks and ends on link loops, and a private `0700` staging cache for temporary copies of documents, swept on open.
- **`kernkraft-pdf` foundation**: load from a path or bytes (encrypted files, files with no pages and malformed files are typed errors; a parser panic is caught), atomic save with external-change detection, strict page-range parsing (out of range is an error, never clamped), and page-tree flattening that keeps every inherited `Resources`, `MediaBox`, `CropBox` and `Rotate`.
- **`kernkraft-pdf` page operations**: extract, split by ranges, per page and by bookmarks (named destinations included), merge, rotate, reorder and remove. Pages keep their inherited attributes through every operation, and pages left out of a result take none of their content with them. Merge, extract and split do not carry bookmarks into their result; remove keeps the outline.
- **`kernkraft-pdf` rendering** (`render` feature): MuPDF renders a page to a requested width as opaque RGBA8; `Renderer` keeps a document open for many pages.

### Results
- 190 tests across 14 suites, all passed (0.1 s warm, CachyOS x86-64, rustc 1.99.1).
