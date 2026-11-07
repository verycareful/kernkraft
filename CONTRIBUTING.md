# Contributing to kernkraft

Bug reports, fixes, features, tests and documentation are all welcome. This file
covers how to report a problem, how to send a change, and the rules a change has to
meet before it is merged.

kernkraft is a Rust library, the shared core of the Kern apps: file operations, text
and PDF handling, and desktop integration. A problem you met in the Kern Desktop app
itself belongs on [its issue tracker](https://github.com/verycareful/kern-desktop/issues/new/choose).

Everyone taking part is expected to follow the [code of conduct](CODE_OF_CONDUCT.md).

## Licence

kernkraft is licensed under AGPL-3.0-or-later, and contributions are accepted under
the same licence: inbound equals outbound. By opening a pull request you agree that
your change is released under AGPL-3.0-or-later as part of kernkraft, and you confirm
that you have the right to submit it. You keep the copyright on what you wrote.

Code copied from another project has to come under a licence compatible with
AGPL-3.0, and the pull request has to say where it came from. A file moved in from
another project keeps its original author in the copyright line.

Every source file starts with an SPDX line and the project's copyright line, and CI
checks the first five lines of each file for both:

```
// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)
```

Use the file's own comment syntax. If you want credit for a file you wrote, add your
own copyright line directly beneath these two.

## Reporting a bug

Open an issue with the **Bug report** form. The form asks for:

- **The crate** the problem is in, and the **kernkraft version** (the release tag, or
  the git revision if you build from a commit).
- **What you use kernkraft from**: Kern Desktop, or your own project.
- **What went wrong**: wrong output, a panic or crash on a malformed file, an edited
  file that differs from the original outside the edit (a round-trip failure), a file
  lost or corrupted, or a problem with the public API.
- **A minimal reproducer**: a few lines of code, or a file that triggers it.
- **What you expected** and **what happened**, with the exact text of any error or
  panic message.

Only attach a file you may redistribute. If the file is private, describe it instead
(its size, how it was made, what it contains).

One problem per issue. Write the title as the symptom and where it shows, for
example `fs: copy into a symlinked directory follows the link twice`. Put the version
in the body, not the title.

An issue with too little to act on gets the `needs info` label and a question. It may
be closed if the details do not follow; reopening it with them is welcome.

Security problems are different: do not open a public issue. See
[SECURITY.md](SECURITY.md).

## Requesting a feature

Open an issue with the **Feature request** form. Start from the problem: what you
were trying to do, and where kernkraft stopped you. A proposed API is optional.
`fs: async API` cannot be acted on; `fs: copying a large tree blocks my UI thread and
I cannot cancel it` can, because it says what the change is for.

kernkraft never opens a network connection, and its core crates never depend on a UI
or platform crate, so a request that needs either is out of scope.

## Labels

Maintainers set labels when triaging. Suggesting them in a report is welcome.

- **Type**: `bug`, `enhancement`, `documentation`, `performance`, `regression`,
  `question`, `api`.
- **Crate**: `crate:fs`, `crate:pdf`, `crate:text`, `crate:xdg-apps`, `crate:thumb`,
  `crate:bus`, `crate:theme`, `crate:volumes`, `crate:tauri`.
- **Kind**: `round-trip` (an edited file differs from the original outside the edit)
  and `malformed-input` (a panic or wrong result on a damaged or hostile file).
- **Severity**, on bugs only, exactly one:
  - `severity:critical`: a file is lost or corrupted, or bytes the edit did not touch
    change on a round trip.
  - `severity:high`: a public API gives wrong results or panics on input it should
    handle.
  - `severity:medium`: a clear failure with a workaround.
  - `severity:low`: cosmetic or minor.
- **Status**: `needs info`, `confirmed`, `out of scope`, `blocked upstream`,
  `duplicate`, `wontfix`.

## Sending a change

1. Fork the repository and branch from `main`. Name the branch for the change:
   `fix/trash-name-collision`, `feat/text-detect-bom`.
2. A bug fix can go straight to a pull request. A feature, any change to a public API,
   or any change to how a file is written needs an issue first, so that the design is
   agreed before the work starts.
3. Make the change, with tests where it adds logic (see the rules below).
4. Run the checks below and make sure they pass. CI runs them on every pull request.
5. Open the pull request against `main`. Describe what changed and why, and link the
   issue.

Do not change the version. Version strings, `CHANGELOG.md` and releases are handled by
the maintainer when the change is merged.

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/):
`type(scope): summary`, with the scope naming the crate.

```
fix(fs): keep the file mode when saving through a symlink
feat(text): detect a UTF-16 byte order mark
test(pdf): page rotation on an inherited attribute
docs(theme): describe the resolution order
```

### Running the checks

You need a stable Rust toolchain and the system libraries the workspace builds
against. The CI workflow (`.github/workflows/ci.yml`) lists the exact Debian package
names: fontconfig, FreeType and libclang for the PDF renderer, the WebKitGTK and GTK 3
development packages for the Tauri adapter, and a D-Bus daemon that the volume tests
start. Then run from the repository root:

```
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features --no-fail-fast
scripts/check-boundary.sh
scripts/check-headers.sh
```

### Changes that also touch Kern Desktop

Send the kernkraft change first. Kern Desktop pins kernkraft by tag, and the
maintainer moves the pin once your change is merged and released.

### Know what you submit

Read every line of your change before you submit it, and be able to explain why each
line is there. Whatever produced the code (you, a generator, an AI tool, another
project), you are the one submitting it, and review questions go to you. A pull
request whose author cannot answer for its contents is closed.

## Rules for code

These hold everywhere in kernkraft. A change that breaks one is not merged.

- **No network.** kernkraft never opens a network connection. `cargo deny` bans HTTP
  and socket crates. Unix-domain sockets, such as the session D-Bus through
  `kernkraft-bus`, are local and allowed; nothing may open an internet socket.
- **Core crates never depend on the platform.** Only the adapter crates
  (`kernkraft-tauri`, and later `kernkraft-jni`) may depend on `tauri`, `jni` or a UI
  crate. `scripts/check-boundary.sh` fails the build otherwise. Long operations take a
  progress and cancel trait, never a UI emitter.
- **Never lose or corrupt a user's file.** Every save goes through `kernkraft-fs`'s
  atomic writer: a temporary file in the same directory, fsync, rename, through
  symlinks, with the mode kept and an outside modification detected.
- **The round trip is the contract.** A part of a file that the code does not model is
  written back as its original bytes. A part it does model keeps every element and
  attribute it does not understand, in order. Only what was edited is re-serialised.
- **Malformed files are expected.** A public API never panics on malformed input; it
  returns the crate's typed error.
- **Formats come from their specifications.** Implement from the specification (for
  example ECMA-376 for OOXML, ISO 32000 for PDF) and cite the section in the file
  header. Do not port code from other suites such as Apache POI or LibreOffice.
- **Constants have a source**: a specification (cited), `std` or the type's limits. No
  bare magic numbers.
- **Tests come with logic.** Unit tests sit beside the code in `#[cfg(test)]`;
  integration and round-trip tests go in each crate's `tests/`, mirroring `src/`.
  Expected values are derived from the specification or the behaviour, never copied
  from a run of the code under test. A file added to `testdata/` must be one the
  project may redistribute.
- **Comments explain the code as it stands**: how it works, why it is there, what
  breaks without it. What changed, when, and why it changed belong in the commit
  message and the pull request, not in a comment.
- **No em dashes and no double hyphens** in code, comments, documentation, commit
  messages or pull request text, except where the syntax needs them (command-line
  flags). Use a colon, a comma, parentheses or two sentences.
- Follow the Rust conventions: crates named `kernkraft-<area>`, `snake_case` modules
  and files, `PascalCase` types, one error enum per crate. `rustfmt` and Clippy have
  no backlog: they pass.

## Good first issues

Issues labelled `good first issue` are small and self-contained. Comment on one before
starting, so that two people do not work on it at once.
