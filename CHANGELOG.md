# Changelog - kernkraft

All notable changes documented here. Format: [Keep a Changelog](https://keepachangelog.com/), with one deviation: dated release entries are timestamped to the minute with timezone (`YYYY-MM-DD HH:MM (TZ)`).

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
