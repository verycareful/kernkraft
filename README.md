# kernkraft
<!-- Language -->
[![Rust](https://img.shields.io/badge/Rust-1.99-000000?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)
<!-- Libraries -->
[![lopdf](https://img.shields.io/badge/lopdf-0.45-informational?style=flat-square)](https://crates.io/crates/lopdf)
<!-- MuPDF has no shields.io logo yet; swap this for a logo badge if one appears. -->
[![mupdf](https://img.shields.io/badge/mupdf-0.6-informational?style=flat-square)](https://crates.io/crates/mupdf)
<!-- Release -->
[![Version](https://img.shields.io/badge/Version-0.1.1.0.6-informational?style=flat-square)](VERSION)
<!-- Licence and status -->
[![License: AGPL-3.0](https://img.shields.io/badge/License-AGPL--3.0--or--later-blue?style=flat-square)](LICENSE)
[![Status](https://img.shields.io/badge/Status-Early%20Development-orange?style=flat-square)](#)

kernkraft is the shared Rust core behind Kern Desktop, Qyra and, later,
Android Kern. It owns everything that touches a user's files: reading and
writing document formats, PDF page operations, and the file operations
around them (atomic save, trash, copy and move). It is a set of plain,
synchronous library crates with no UI and no network access; each
application wraps them in its own adapter.

kernkraft is in alpha: its public API will change.

## Crates

| Crate | Kind | Owns |
|---|---|---|
| `kernkraft-fs` | core | atomic save, FreeDesktop trash, XDG directories, bulk file operations, scanning |
| `kernkraft-pdf` | core | PDF load and atomic save, page ranges, page-tree flattening, extract, split, merge, rotate, reorder, remove; MuPDF rendering (`render` feature) |
| `kernkraft-text` | core | text decode, UTF-8 BOM and line-ending preservation, read-only reasons, atomic save |
| `kernkraft-xdg-apps` | core | MIME type lookup (Shared MIME-info Database), desktop entries, default applications |

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

Every file is licensed under AGPL-3.0-or-later. Files holding code moved
from [Qyra](https://github.com/zParik/Qyra) also name the Qyra authors as
copyright holders in their header, since they wrote it.

See the [LICENSE](LICENSE) file for the full license text.
