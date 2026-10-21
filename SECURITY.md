# Security policy

## Reporting a vulnerability

Report a vulnerability through GitHub's private advisory form:
https://github.com/GF6599/blank/security/advisories/new

Do not open a public issue for one. `blank` destroys data, so a working bypass
published before a fix is a working attack.

Expect an acknowledgement within seven days.

## What counts as a vulnerability

`blank` refuses to destroy the root of a filesystem, the directories an
operating system installs itself into, the home directory, and any directory
that holds one of those. A path that reaches destruction through that guard is
a vulnerability. So is any route that makes the tool write to a target the
operator did not name.

The following are known properties rather than vulnerabilities. Each one is
described in the README.

- An overwrite does not always reach the bytes a file held. An SSD remaps
  blocks, and a copy-on-write filesystem may leave the old blocks intact. This
  limit belongs to the storage layer, and no number of passes removes it.
- The guard resolves symlinks and does not resolve mount points. A bind mount
  of a system path inside the home directory is a second name the guard cannot
  see. The guard catches a mistyped argument, which is what it is for.
- A file with a hard link outside the selection is left intact unless
  `--allow-hard-links` is given, because covering it would destroy the contents
  under names nobody selected. Anyone who can write in a selected tree can add
  such a link to a file the operator owns. The run reads the link count when it
  opens each file, so a link added after the plan is caught too. With the flag,
  the contents under every name are destroyed.

## Supported versions

The most recent release receives fixes. Nothing older does.
