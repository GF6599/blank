# 🧽 blank

`blank` destroys files and directories so that the contents cannot be recovered. On a
terminal it runs full screen: a selector with a preview of each entry, a review of every
target before anything is destroyed, and a live view of the run. From a script it prints
one line per target.

> **Destruction is permanent. `blank` has no undo and no trash can. Read "Known
> limitations" before you point it at anything you care about.**

## How it works

`blank` reads the whole plan before it changes anything. It expands each argument into a
list of targets and records the device and inode of each. Then it overwrites every file
on parallel workers, unlinks every symlink and special file, and removes every directory,
deepest first.

Each file is written over in place: `--passes` random covers, 10 by default, then one zero
cover, with a flush to the device after each. The name is then replaced with a meaningless
one of the same length before the entry is unlinked, because a directory entry carries the
filename long after the contents are gone. Before each step, the run checks that the entry
under that name is still the file the plan found.

A symlink and a special file hold no contents of their own, so `blank` unlinks them and
never opens them. A symlink is never followed, so naming one destroys the link and leaves
the file it points at alone.

A directory is removed only when it is empty. A directory that still holds something is
reported and left on the disk. That is what stops a file whose overwrite failed from being
unlinked with its contents intact.

## Before it starts

**It refuses a path that holds everything else.** The root of the filesystem, the
directories an operating system installs itself into, your home directory, and any
directory that holds one of those. One refused argument stops the whole command line,
because the arguments beside a mistyped one are not to be trusted either. There is no flag
to override this. The answer to a refusal is to name a narrower path.

```console
$ blk ~
Error: /Users/dev is a protected path, which blank refuses to destroy
```

**`--dry-run` lists every target and destroys nothing.** It prints the plan in the order
the run reaches each target, then the statistics: what kinds of target there are, how many
bytes the files hold, and how many bytes the covers will write.

```console
$ blk --dry-run ./old-project
Overwriting /Users/dev/old-project/archive/photo.raw
Overwriting /Users/dev/old-project/keys/id_ed25519
Overwriting /Users/dev/old-project/notes.txt
Removing /Users/dev/old-project/link
Deleting directory /Users/dev/old-project/keys
Deleting directory /Users/dev/old-project/archive
Deleting directory /Users/dev/old-project
Dry run. 3 files, 1 symlink or special file, and 3 directories. Nothing was destroyed.
The files hold 293.1 KiB (300092 bytes), and the covers will write 3.1 MiB (3301012 bytes) in 11 passes.
```

**It asks before it destroys anything.** On a terminal, the review screen asks. From a
script, the question is a line on standard output. Only a whole `yes` goes on. Anything
else stops the run and exits non-zero, and so does a closed input, so a command in a
pipeline refuses and does not wait. Pass `--yes` to skip the question.

```console
$ blk ./old-project < /dev/null
blank will destroy 3 files, 1 symlink or special file, and 3 directories.
The files hold 293.1 KiB (300092 bytes), and the covers will write 3.1 MiB (3301012 bytes) in 11 passes.
Selected roots:
  /Users/dev/old-project
This cannot be undone. Type yes to continue:
Error: cancelled, nothing was destroyed
```

## The full screen

`blank` runs full screen when standard input and standard output are both terminals. The
run view stays up after the run ends until you press `q`. The verdict then prints to the
terminal, so the result stays in your scrollback. `--no-ui` turns the screen off.

### Choosing targets

Run `blk` with no arguments and it opens a selector on the working directory. Mark what you
want and press `enter`. The marked set then goes through the same guard, the same plan and
the same review as a command line.

```text
  4/4  1 marked                           [space] mark  [enter] done  [q] quit
  marked: 1 file, 1 directory, 293.0 KiB

  ┌─ /Users/dev/old-project ───────────┐ ┌─ preview ─────────────────────────┐
  │   * archive/                       │ │ Meeting notes                     │
  │     keys/                          │ │ - move the backups                │
  │     link@                          │ │ - rotate the keys                 │
  │ [   notes.txt                   ]  │ │                                   │
  └────────────────────────────────────┘ └───────────────────────────────────┘
```

The preview pane shows the entry under the cursor. A text file shows its first lines, a
binary file shows a hex dump of its first bytes, a directory shows what marking it would
destroy, and a symlink shows where it points. An image (PNG, JPEG, GIF, WebP, BMP or TIFF,
told by its first bytes and not by its name) shows as the picture under a caption with its
format and its size in pixels. The picture goes through the terminal's graphics protocol,
kitty, iTerm2 or sixel, and through coloured half-blocks on a terminal with none.

The line under the status row totals the whole marked set. From 124 columns a third pane,
`marked`, takes its place and shows the statistics the review shows: each kind, the bytes
the files hold and the bytes the covers will write, the five largest files, and the files
the run will leave.

| Cell | Meaning |
| --- | --- |
| `*` | Marked. The run will destroy this. |
| `-` | Refused. This is a protected path, so no key marks it. |
| blank | Not marked. |

A name that ends in `/` is a directory, and marking one destroys everything under it. A
name that ends in `@` is a symlink, and marking one destroys the link and never what it
points at.

| Key | Action |
| --- | --- |
| `space` | Mark or unmark the entry under the cursor |
| `l`, `right` | Go into the directory under the cursor |
| `h`, `left`, `backspace` | Go up one directory, and stop at a protected one |
| `j`, `k`, `up`, `down` | Move the cursor |
| `pgup`, `pgdown`, `home`, `end` | Move the cursor by a page, or to an end |
| `/` | Filter this directory by name |
| `enter` | Done. Hand the marked set to the review |
| `q`, `ctrl+c` | Quit. Destroy nothing |
| `esc` | Clear the filter, or quit when there is none |

A mark holds while you browse, so targets in several directories go to one run.

**The selector never lists a protected directory.** It refuses to open in one, and the up
key stops at the first one above you. With no arguments and no terminal, `blk` prints the
usage, so a script never waits on a screen nobody can answer.

```console
$ cd ~ && blk
Error: /Users/dev is a protected path, which blank refuses to destroy, so it will not open
a selector there either. Name a path on the command line, or change to a narrower
directory first
```

### Reviewing the plan

Before anything is destroyed, the review screen shows every target as a tree with its size,
the statistics of the plan, and a preview of the target under the cursor. Type `yes` and
press `enter` to destroy them. `q` quits and destroys nothing, and so does `esc` when
nothing is typed.

```text
  blank will destroy 1 file, 0 symlinks and special files, and 1 directory.
  The files hold 293.0 KiB (300000 bytes), and the covers will write 3.1 MiB
  (3300000 bytes) in 11 passes.
  Type yes and press enter to destroy them:                [jk] move  [q] quit

  ┌─ targets ──────────────────────────┐ ┌─ preview ─────────────────────────┐
  │ [   ...oject/archive/ 293.0 KiB ]  │ │ the run destroys                  │
  │       photo.raw       293.0 KiB    │ │ 1 file                            │
  │                                    │ │ 1 directory                       │
  │                                    │ │ 293.0 KiB                         │
  └────────────────────────────────────┘ └───────────────────────────────────┘
```

A file with more than one name carries a `!` and `hard-linked`, because the run leaves it
intact. See "Known limitations".

### Watching the run

```text
  ████████████████████████████████████ 100%  [/] filter  [jk] scroll  [q] quit
  blanking complete

  ┌─ activity ───────────────────────────────────────────────────────────────┐
  │     Overwriting /Users/dev/old-project/archive/photo.raw                 │
  │     Deleting directory /Users/dev/old-project/archive                    │
  └──────────────────────────────────────────────────────────────────────────┘
```

The bar weighs a file by the bytes its covers write, plus one unit for the unlink that ends
it. Only work that finished counts, so a planned target that survives keeps the bar below
100%. The status line, not the bar, is the verdict. The activity panel holds the last 200
rows. Pass `--log <path>` for the full
manifest of a larger run.

| Cell | Meaning |
| --- | --- |
| `⠋` | The overwrite is running. |
| blank | The target is destroyed. |
| `!` | The target survived. The run will exit non-zero and name it. |

| Key | Action |
| --- | --- |
| `q`, `ctrl+c` | Stop the run |
| `esc` | Clear the filter, or stop the run when there is none |
| `/` | Filter the activity panel by name |
| `j`, `k`, `up`, `down` | Scroll one row |
| `f`, `b`, `pgdown`, `pgup` | Scroll half a page |

The panel follows the newest row until you scroll away, and follows again once you scroll
back to it. The filter narrows what the panel draws and never what the run does. A stop
takes effect before each worker writes its next megabyte. Files already overwritten stay
destroyed.

## What happens when a target is not destroyed

`blank` exits non-zero and names every target it did not destroy, on standard error, in
both modes. A target is left behind when the walk cannot list a directory, when an
overwrite fails or is refused, or when a directory is still not empty when the run reaches
it. Each name carries the state the target was left in:

| State | What is on the disk |
| --- | --- |
| `intact` | The target still holds everything it held. Its mode can be wider, because an overwrite forces the mode to `0600` once when the first open is refused. |
| `damaged, the contents are gone` | The entry is still there and its contents are not. At least one cover landed, so what it held is unrecoverable. |

```console
$ blk --yes --no-ui ./old-project
...
Error: 2 targets were not destroyed
  /Users/dev/old-project/keys: read directory: Permission denied (os error 13) (intact)
  /Users/dev/old-project: remove directory: Directory not empty (os error 66) (intact)
```

A stopped run over a large file is the usual way to reach the damaged state. The covers are
written a megabyte at a time, so a stop lands in the middle of one, and every file in
flight at that moment is damaged.

| Status line | Exit code |
| --- | --- |
| `blanking` | The run is still going. |
| `blanking complete` | 0. Every target was destroyed. |
| `blanking failed, 2 targets survived` | 1. Standard error names them and their state. |
| `blanking cancelled` | 1. Standard error names every target the run did not destroy. |

A refused question, a refused path, a selector left with nothing marked, and a log that
cannot be opened all exit non-zero having destroyed nothing. So does a dry run over a
directory it could not list, because its plan is missing what that directory holds.

## Flags

| Flag | Effect |
| --- | --- |
| `--dry-run` | List every target and the statistics, and destroy nothing. |
| `--yes`, `-y` | Destroy without asking first. |
| `--jobs <n>` | Files to overwrite at the same time. Default 16. Zero is refused. |
| `--passes <n>` | Random covers over each file before the zero cover. Default 10. |
| `--log <path>` | Append the state of every target to this file. Off by default. |
| `--allow-hard-links` | Destroy a file that has more than one name, under every name. |
| `--no-ui` | Turn off the full screen and print one line per target. |
| `--version`, `-V` | Print the version, the commit, and the commit date. |

The work is disk bound, so the best `--jobs` value belongs to the device and not to the
processor count. Each worker holds one buffer of 1 MiB. Measure your own disk before you
change it.

The log names the full path of every target, so it is a manifest of what you destroyed and
where it lived. It is mode `0600` whether the run creates it or appends to a file already
there. A path that is a symlink or not a regular file stops the run before it destroys
anything, and so does a path inside a selected target, because the run would destroy its
own manifest. A dry run and a refused question never open the log.

## Install

On macOS, through Homebrew:

```sh
brew install --cask GF6599/tap/blank
```

Each release also carries a `tar.gz` for darwin and linux on amd64 and arm64, with
`checksums.txt` beside them. The Linux binaries link against glibc.

From source, with Rust 1.99 or later:

```sh
cargo install --locked --git https://github.com/GF6599/blank
```

`just install` builds the binary and writes it to `~/self-made-bin/blank`. That directory
is not on `PATH` by default, so add an alias:

```sh
alias blk="$HOME/self-made-bin/blank"
```

`blank --version` names the build, which is what a bug report needs.

## Known limitations

These are properties of the current implementation. Each one is reproducible.

- **A file with more than one name is left intact.** A hard link is the file, not a pointer
  to it, so covering one name destroys the contents under every other name too, including
  names outside the selection. The run reads the link count of each file before it changes
  anything, and again once the file is open. A file with more than one name is reported as
  `intact`, with its mode unchanged. The statistics count such files, and the review marks
  each one. Pass `--allow-hard-links` to destroy the contents under every name. The rule
  holds when every name is selected too, because a count of the selected names can be
  fooled by a name removed after the plan.
- **A freed inode can be reused.** The run checks the device and inode of every target
  before it acts. A file deleted and created again under the same name between the review
  and the run can receive the inode just freed, and is then destroyed as the planned file.
  Only a new file can take a freed inode, so this cannot reach a file outside the
  selection.
- **The parent of each root is opened by path.** Everything below a root is opened
  relative to its parent and never through a symlink. The step to the parent of the root
  itself follows the path you named. The guard checks the resolved parent once, before the
  plan, and not again when the run starts, so a writer above the selection can redirect
  that one step.
- **Writing over a file does not always reach the bytes it held.** An SSD remaps blocks as
  it writes, and a copy-on-write filesystem such as APFS, Btrfs or ZFS can place the new
  data somewhere else and leave the old blocks intact. A snapshot, a journal, or a backup
  keeps its own copy either way. No number of passes removes this limit. Full disk
  encryption is the answer that does.
- **A name is escaped, but a lookalike letter is not caught.** Every control character,
  every default ignorable code point, every format character, and the line and paragraph
  separators display as their code points, read from the Unicode data. So an invisible or
  reordering character cannot make one name pass for another. An emoji with a variation
  selector therefore shows the selector as `\u{fe0f}`. A letter from another script that
  looks like a Latin one, such as the Cyrillic `а`, displays as itself.
- **An image preview decodes an untrusted file inside blank.** The decoders are written in
  Rust and link no C library, and two caps bound what one preview costs: a file over
  32 MiB, or an image whose header claims pixels that would take more than 128 MiB, shows
  its first bytes and the reason instead. The size check reads the header before any pixel
  is decoded, so a small file that claims a huge image costs nothing.
- **The screen asks the terminal how it draws images.** It asks once a session, when the
  selector or the review first opens, and waits at most half a second for the answer. A
  terminal that never answers the device status request then loses the first keystroke
  typed after the screen opens, because the query keeps reading for it. Inside tmux the
  query also turns on `allow-passthrough` for the pane, so the images reach the outer
  terminal.
- **A directory the walk cannot list gets no row and no weight.** It is not in the plan, so
  the activity panel never shows it, and the bar can reach 100% while the status line says
  `blanking failed, 1 target survived`. The error after the screen closes names it as
  `intact`.
- **The selector does not go into a symlink.** It reads a symlink the way the run does, so
  the arrow key does nothing on one. Name the path on the command line to reach what is
  behind it.
- **The selector is confined to the tree below the nearest protected directory.** From
  `~/projects` you can browse anything under it, and you cannot go up into your home
  directory. Name a path on the command line to reach anything higher.
- **The protected path guard resolves symlinks, but not mount points.** It reads the path
  you typed, and the same path with every symlink above the last component resolved,
  against a list and against your home directory. A bind mount of a system path inside
  your home is a second name the resolution cannot see. The guard catches a mistyped
  argument, which is what it is for. It is not a barrier against someone determined to
  destroy their own machine.

## Development

```sh
just          # list every recipe
just check    # the gate: format check, clippy for the host and x86_64 Linux, cargo deny, tests, build
just run      # open the selector on the working directory
just e2e      # generate a dummy tree, destroy it, and check what survived
```

`just check` is the gate that CI runs on Ubuntu and macOS. Run it before every commit and
read its exit code. `rust-toolchain.toml` pins the toolchain, and rustup installs it at the
first `cargo` call.

### Releasing

A release is a semver tag on a commit whose `Cargo.toml` names the same version. The
release workflow stops when the two differ, because the binary reports the version in
`Cargo.toml` and GoReleaser names the release after the tag.

1. Set the version in `Cargo.toml`, run `cargo build` to update `Cargo.lock`, and commit
   both as `🔖 Release X.Y.Z`.
2. Run `just check`, and confirm `cargo run -- --version`.
3. Push `main`, then push a tag `vX.Y.Z` on that commit.

The tag runs the gate on both platforms, then GoReleaser builds the four archives on macOS,
writes `checksums.txt`, publishes the GitHub release, and updates the cask in
`GF6599/homebrew-tap`. Run the Release workflow by hand to render everything a tag would
publish and publish none of it. The run keeps the archives and the cask as an artifact.
Prove a toolchain or action upgrade there before you tag.
