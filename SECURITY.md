# Security Policy

## Reporting a vulnerability

Report it privately through GitHub: open the repository's **Security** tab and choose
**Report a vulnerability**, or go to
<https://github.com/verycareful/kernkraft/security/advisories/new>. Do not open a
public issue, pull request or discussion about it.

Include the crate, the kernkraft version (the release tag, or the git revision), and
a reproducer: the file or the few lines of code that trigger the problem.

kernkraft is maintained by one person. I aim to acknowledge a report within 7 days,
and I cannot promise a date for the fix. Once it is fixed I publish a GitHub security
advisory, crediting you if you want to be named.

## What counts

kernkraft reads and writes files it did not create, and the apps built on it open
documents from anywhere, so a crafted file is the main attack surface. Examples of
what to report:

- a file that makes a kernkraft crate execute code, write outside the file it was
  given, or read a file it was not asked to read;
- memory unsafety reachable from a file, including through the PDF renderer (MuPDF is
  C code underneath);
- a way past the atomic save or the temporary-copy handling, such as a symbolic link
  that redirects a write, or a temporary copy another user can read;
- any network connection made by a kernkraft crate.

A panic or crash on a malformed file with no further effect is a bug: report it with
the bug report form. So is a file lost or changed on save, unless a crafted file can
trigger it from outside, in which case report it privately.

## Out of scope

- An attacker who can already run code as the user. kernkraft runs with the caller's
  permissions and is not a sandbox.
- Bugs in a dependency such as MuPDF or Tauri. Report them upstream. A note to me is
  welcome if you think kernkraft is exposed.

## Supported versions

kernkraft is in alpha. Fixes go into the latest release only.
