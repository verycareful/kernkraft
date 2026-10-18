# Changelog - kernkraft

All notable changes documented here. Format: [Keep a Changelog](https://keepachangelog.com/), with one deviation: dated release entries are timestamped to the minute with timezone (`YYYY-MM-DD HH:MM (TZ)`).

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
