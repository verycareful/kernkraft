# kernkraft
[![Rust](https://img.shields.io/badge/Rust-1.99-000000?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Tauri](https://img.shields.io/badge/Tauri-2.12-FFC131?style=flat-square&logo=tauri&logoColor=white)](https://tauri.app/)
[![image](https://img.shields.io/badge/image-0.25-informational?style=flat-square)](https://crates.io/crates/image)
[![resvg](https://img.shields.io/badge/resvg-0.48-informational?style=flat-square)](https://crates.io/crates/resvg)
[![png](https://img.shields.io/badge/png-0.18-informational?style=flat-square)](https://crates.io/crates/png)
[![simplecss](https://img.shields.io/badge/simplecss-0.2-informational?style=flat-square)](https://crates.io/crates/simplecss)
[![md-5](https://img.shields.io/badge/md--5-0.10-informational?style=flat-square)](https://crates.io/crates/md-5)
[![rustix](https://img.shields.io/badge/rustix-1-informational?style=flat-square)](https://crates.io/crates/rustix)
[![jiff](https://img.shields.io/badge/jiff-0.2-informational?style=flat-square)](https://crates.io/crates/jiff)
[![tempfile](https://img.shields.io/badge/tempfile-3-informational?style=flat-square)](https://crates.io/crates/tempfile)
[![inotify](https://img.shields.io/badge/inotify-0.11-informational?style=flat-square)](https://crates.io/crates/inotify)
[![zbus](https://img.shields.io/badge/zbus-5-informational?style=flat-square)](https://crates.io/crates/zbus)
[![percent-encoding](https://img.shields.io/badge/percent--encoding-2-informational?style=flat-square)](https://crates.io/crates/percent-encoding)
[![thiserror](https://img.shields.io/badge/thiserror-2-informational?style=flat-square)](https://crates.io/crates/thiserror)
[![serde](https://img.shields.io/badge/serde-1-informational?style=flat-square)](https://crates.io/crates/serde)
[![serde_json](https://img.shields.io/badge/serde_json-1-informational?style=flat-square)](https://crates.io/crates/serde_json)
[![base64](https://img.shields.io/badge/base64-0.22-informational?style=flat-square)](https://crates.io/crates/base64)
[![getrandom](https://img.shields.io/badge/getrandom-0.3-informational?style=flat-square)](https://crates.io/crates/getrandom)
[![flate2](https://img.shields.io/badge/flate2-1-informational?style=flat-square)](https://crates.io/crates/flate2)
[![weezl](https://img.shields.io/badge/weezl-0.2-informational?style=flat-square)](https://crates.io/crates/weezl)
[![aes](https://img.shields.io/badge/aes-0.9-informational?style=flat-square)](https://crates.io/crates/aes)
[![cbc](https://img.shields.io/badge/cbc-0.2-informational?style=flat-square)](https://crates.io/crates/cbc)
[![rc4](https://img.shields.io/badge/rc4-0.2-informational?style=flat-square)](https://crates.io/crates/rc4)
[![sha2](https://img.shields.io/badge/sha2-0.10-informational?style=flat-square)](https://crates.io/crates/sha2)
[![unicode-normalization](https://img.shields.io/badge/unicode--normalization-0.1-informational?style=flat-square)](https://crates.io/crates/unicode-normalization)
[![unicode-linebreak](https://img.shields.io/badge/unicode--linebreak-0.1-informational?style=flat-square)](https://crates.io/crates/unicode-linebreak)
[![unicode-bidi](https://img.shields.io/badge/unicode--bidi-0.3-informational?style=flat-square)](https://crates.io/crates/unicode-bidi)
[![moxcms](https://img.shields.io/badge/moxcms-0.8-informational?style=flat-square)](https://crates.io/crates/moxcms)
[![tiny-skia](https://img.shields.io/badge/tiny--skia-0.12-informational?style=flat-square)](https://crates.io/crates/tiny-skia)
[![zune-jpeg](https://img.shields.io/badge/zune--jpeg-0.5-informational?style=flat-square)](https://crates.io/crates/zune-jpeg)
[![hayro-jpeg2000](https://img.shields.io/badge/hayro--jpeg2000-0.5-informational?style=flat-square)](https://crates.io/crates/hayro-jpeg2000)
[![hayro-jbig2](https://img.shields.io/badge/hayro--jbig2-0.4-informational?style=flat-square)](https://crates.io/crates/hayro-jbig2)
[![hayro-ccitt](https://img.shields.io/badge/hayro--ccitt-0.5-informational?style=flat-square)](https://crates.io/crates/hayro-ccitt)
[![skrifa](https://img.shields.io/badge/skrifa-0.44-informational?style=flat-square)](https://crates.io/crates/skrifa)
[![read-fonts](https://img.shields.io/badge/read--fonts-0.41-informational?style=flat-square)](https://crates.io/crates/read-fonts)
[![subsetter](https://img.shields.io/badge/subsetter-0.2-informational?style=flat-square)](https://crates.io/crates/subsetter)
[![jni](https://img.shields.io/badge/jni-0.21-informational?style=flat-square)](https://crates.io/crates/jni)
[![Version](https://img.shields.io/badge/Version-0.2.3.0-informational?style=flat-square)](VERSION)
[![License: AGPL-3.0](https://img.shields.io/badge/License-AGPL--3.0--or--later-blue?style=flat-square)](LICENSE)
[![Status](https://img.shields.io/badge/Status-Early%20Development-orange?style=flat-square)](#)

<!-- Plain badges, because shields.io has no logo for them yet: image, resvg, png, simplecss, md-5, rustix, jiff, tempfile, inotify, zbus, percent-encoding, thiserror, serde, serde_json, base64, getrandom, flate2, weezl, aes, cbc, rc4, sha2, unicode-normalization, unicode-linebreak, unicode-bidi, moxcms, tiny-skia, zune-jpeg, hayro-jpeg2000, hayro-jbig2, hayro-ccitt, skrifa, read-fonts, subsetter, jni. Swap each for a logo badge if one appears. -->

kernkraft is the shared Rust core behind Kern Desktop and Kern Android.
It owns everything that touches a user's files: reading and
writing document formats, a PDF library of its own (reading, editing,
saving, text, fonts and rendering), and the file operations
around them (atomic save, trash, copy and move). It is a set of plain,
synchronous library crates with no UI and no network access; each
application wraps them in its own adapter.

kernkraft is in alpha: its public API will change.

## Crates

| Crate | Kind | Owns |
|---|---|---|
| `kernkraft-fs` | core | atomic save, FreeDesktop trash, XDG directories, bulk file operations, scanning and properties totals, undo journal for file operations, and the same-filesystem query that tells a move from a copy (device number and mount ID) |
| `kernkraft-pdf` | core | a PDF library written from ISO 32000-1: reading (cross-reference and object streams, repair, encryption with the standard handler, revisions 2 to 6), a document model that never changes, a full writer and an incremental one, an edit session with undo and redo, page ranges, extract, split, merge, rotate, reorder and remove with the outline, destinations, form, tagged structure and page labels kept in agreement, document features (labels, outline, attachments, signatures, revisions, links, active content), the text layer and search in logical order, annotations, text editing, font embedding, and page rendering to RGBA (paths, text, images, shadings, patterns, transparency, annotation appearances). DeviceCMYK goes through the file's output intent, a profile the application gives, or a bundled FOGRA39L profile |
| `kernkraft-font` | core | font programs (TrueType, OpenType, bare CFF, Type 1) with glyph lookup, outlines and metrics, a font list built from the files an application names and matched by the CSS Fonts Level 4 order, variable-font instances, and subsetting |
| `kernkraft-pdf-engine` | core | the PDF session that both adapters wrap: open documents behind handles, typed requests and answers, cancellable and observable long operations, change detection by file stamp |
| `kernkraft-text` | core | text decode, UTF-8 BOM and line-ending preservation, read-only reasons, atomic save |
| `kernkraft-xdg-apps` | core | MIME type lookup (Shared MIME-info Database), desktop entries, default applications, launching through `Exec` lines, and terminal discovery |
| `kernkraft-thumb` | core | image thumbnails (PNG, JPEG, WebP, GIF, SVG) and the freedesktop thumbnail cache shared with other file managers |
| `kernkraft-bus` | core | D-Bus session and system connections that resolve the bus address themselves and only ever use a Unix socket |
| `kernkraft-theme` | core | system theme sources (Hyprland `hyprtoolkit.conf`, portal appearance, GTK, fontconfig), Kern's design tokens and the token resolver that emits CSS custom properties |
| `kernkraft-volumes` | core | mounted volumes from `/proc/self/mountinfo` (labels from udev), UDisks2 mount, unmount and eject over the system bus, and a watcher that reports mounts, unmounts and devices appearing or going away (mountinfo `poll(2)` and UDisks2 signals) |
| `kernkraft-tauri` | adapter | Tauri 2 plugin `kernkraft`: wire types for paths (`WirePath`, lossless for non-UTF-8 names) and errors (`WireError`); file manager commands (listing, create, rename, background copy, move, trash and delete with progress and conflict prompts, trash view, undo and redo, folder watching through inotify, properties); text open, check and save; a thumbnail queue; the system theme with live updates; applications, default apps and the terminal; volumes, listed with an identity the commands check, and updated live through `kernkraft://volumes-changed` (once-only watcher problems held until `volumes_problems_take`); user folders; the same-filesystem query for drops; streamed name search and the Documents walk; PDF open, edit, undo, save, render, merge, extract and split with cancel and progress by job number |
| `kernkraft-jni` | adapter | JNI library `libkernkraft_jni.so` for Kern Android: the PDF session as JSON requests and answers over `dev.kern.pdfbridge.KernkraftPdf`, built for the three Android ABIs |

Core crates never depend on a UI or platform crate (`tauri`, `jni`); CI
checks every core crate's dependency tree. No crate may depend on an HTTP,
socket or async runtime crate; `cargo deny` enforces it.

## Building

```bash
cargo build --workspace
cargo test --workspace --all-features
```

## License

Copyright © 2026 Sricharan Suresh (github.com/verycareful)

kernkraft is licensed under the **[GNU Affero General Public License v3.0 or later](https://www.gnu.org/licenses/agpl-3.0.html)**.
You may use, study, modify and redistribute it freely, under version 3 of the
licence or any later version the Free Software Foundation publishes. Any
derivative work must also be released under the AGPL, including its source.
Unlike the plain GPL, the AGPL extends that obligation to network use: if you
run a modified version and let others interact with it over a network, you
must offer them the source of your modified version.

Every file is licensed under AGPL-3.0-or-later, except the data taken from
other projects (a CMYK colour profile, glyph lists, character maps, font
metrics and Unicode data), which keeps its own licence and is listed in
[NOTICE](NOTICE).

See the [LICENSE](LICENSE) file for the full license text.
