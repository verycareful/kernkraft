## What changed and why

<!-- What the change does, and the problem it solves. -->

Closes #

## How it was checked

<!-- The tests added or changed, including any round-trip or malformed-input cases. -->

## Checklist

- [ ] I have read every line of this change and can explain it.
- [ ] The checks in CONTRIBUTING.md pass.
- [ ] Logic comes with tests, and any test file added is one the project may redistribute.
- [ ] No network access, and no dependency that opens a socket.
- [ ] Core crates do not depend on `tauri`, `jni` or UI crates.
- [ ] Saves go through `kernkraft-fs`'s atomic writer, and parts of a file the edit did not touch are written back as they were.
- [ ] Every new constant cites its source.
- [ ] New files carry the SPDX and copyright header.
- [ ] No version strings or `CHANGELOG.md` changed.
- [ ] Commits follow Conventional Commits.
