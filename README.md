# 🧽 blank

`blank` destroys files and directories so that the contents cannot be recovered. It gives
you a Cobra command line and a Bubble Tea terminal interface that shows live progress and
an activity log.

> **Destruction is permanent. `blank` has no undo and no trash can. Read "Known
> limitations" before you point it at anything you care about.**

## How it works

`blank` expands each argument into a list of files and directories. It overwrites every
file concurrently, then removes every directory.

```mermaid
graph TD
    A[main.go] --> B[cmd: Cobra root command]
    B --> C{--no-ui}
    C -->|off| D[tui: Bubble Tea interface]
    C -->|on| E[stdout printer]
    D --> F[app: deletion engine]
    E --> F
    F --> G["shred -uzfn 10, once per file"]
    F --> H["rm -rf, once per directory"]
    F -. progress and activity .-> D
    F -. progress and activity .-> E
```

Each file gets 10 random overwrite passes plus a final zero pass, which is what
`shred -uzfn 10` performs. Up to 100 files run at the same time.

## What happens when a target survives

`blank` exits non-zero and names every target it did not destroy, on stderr, in both
modes. A target survives when the walk cannot list a directory, when the overwrite exits
non-zero, or when the directory removal fails.

```console
$ blk ./tree
Error: 2 targets survived the run
  /home/dev/tree/locked: read directory: open /home/dev/tree/locked: permission denied
  /home/dev/tree: rm -rf failed: exit status 1 (output: rm: /home/dev/tree: Directory not empty)
$ echo $?
1
```

The status line under the progress bar reports the same verdict, so the interface and the
exit code cannot disagree.

| Status line | Exit code |
| --- | --- |
| `blanking` | The run is still going. |
| `blanking complete` | 0. Every target was destroyed. |
| `blanking failed, 2 targets survived` | 1. The error names them on stderr. |
| `blanking cancelled` | The run stopped before it finished. |

Read the exit code rather than the progress bar. The bar counts targets attempted rather
than targets destroyed, so it reaches 100% on a run that left plaintext on disk.

## Requirements

| Dependency | Why | Note |
| --- | --- | --- |
| Go 1.24.2 or later | To build the binary. | |
| GNU `shred` on `PATH` | The engine runs it once per file. | macOS does not supply it. Run `brew install coreutils`, which puts `shred` on `PATH` under its own name. |
| `rm` on `PATH` | The engine runs it once per directory. | Standard on Linux and macOS. |

On macOS you do not need the `gnubin` directory that `brew info coreutils` describes.
Homebrew adds the `g` prefix only to commands macOS already provides, and macOS has no
`shred`, so you get both `shred` and `gshred`.

Windows has neither program, so there is no Windows release. A Windows build would start
and then fail on every file.

## Install

```sh
just install
```

This builds the binary and writes it to `~/self-made-bin/blank`. That directory is not on
`PATH` by default, so add an alias:

```sh
alias blk="$HOME/self-made-bin/blank"
```

## Usage

```sh
blk ./notes.txt ./old-project
```

The interface renders inline rather than taking over the screen, so the final frame stays
in your scrollback after the program exits. The activity panel holds the last 200 lines
and scrolls. Pass `--log` when you want the full manifest of a larger run.

Each row carries one target, and the cell before the name says how far that target has
got.

| Cell | Meaning |
| --- | --- |
| `⠋` | The overwrite is running. The animation only moves while work is in flight. |
| blank | The target is destroyed. |
| `!` | The target survived. The run will exit non-zero and name it. |

A row settles the moment its own target settles, so what is left marked is what still
needs reading. The panel shows as many rows in flight as `--jobs` allows.

### Keys

| Key | Action |
| --- | --- |
| `q`, `esc`, `ctrl+c` | Cancel the run and quit |
| `k`, `up` | Scroll the activity panel up one line |
| `j`, `down` | Scroll the activity panel down one line |
| `b`, `pgup` | Scroll up half a page |
| `f`, `pgdown` | Scroll down half a page |

`ctrl+c` cancels the context. Files already overwritten stay destroyed.

### Flags

| Flag | Effect |
| --- | --- |
| `--jobs` | Targets to overwrite at the same time. Default 16. Below 1 is refused. |
| `--log` | Write logs to `blank.log` in the working directory. Off by default. |
| `--no-ui` | Hidden. Turn off the terminal interface and print progress to stdout. |

The work is disk bound, so the best `--jobs` value belongs to the device and not to the
processor count. Over 120 files of 1 MiB on an NVMe disk, 16 workers took 136ms, 8 took
143ms, 100 took 177ms, and 4 took 216ms. Measure your own disk before you change it.

`blank.log` records the full path of every file the run touched, so treat it as a manifest
of what you destroyed. It is created mode `0600`, readable only by you.

## Known limitations

These are properties of the current implementation. Each one is reproducible.

- **Scrolling does not hold during a run.** Every new target jumps the panel back to the
  newest row, so `k` and `j` only stay put once the run has finished.
- **A target that fails before the walk queues it gets no row.** A directory the walk
  cannot list is counted in the status line and named in the error, but no row in the
  panel carries a `!` for it, because the run never reached it.
- **A symlink argument destroys the target.** A symlink passed on the command line goes to
  `shred`, which follows it. A symlink found inside a walked directory is removed as a
  link, so only the top level is affected.
- **A FIFO blocks the run.** `shred` opens the FIFO and waits for a writer that never
  arrives.
- **There is no confirmation prompt and no dry run.** The program starts destroying as
  soon as it parses the arguments, and it refuses no path.

## Development

```sh
just          # list every recipe
just check    # the full gate: format check, vet, staticcheck, race tests, build
just run .    # go run . with the local build cache
just e2e      # generate a dummy tree and delete it headlessly
```

`just check` is the gate. Run it before every commit and read its exit code.

| Recipe | Purpose |
| --- | --- |
| `fmt`, `fmt-check` | Rewrite or report `gofmt` drift. |
| `lint` | `go vet` plus `staticcheck`. |
| `test`, `test-race` | Run the suite, with or without the race detector. |
| `build`, `build-race` | Write the binary to `bin/`. |
| `release <os> <arch>`, `release-all` | Cross compile into `dist/`. |
| `checksums` | Write `dist/SHA256SUMS`. |
| `clean` | Remove `bin/`, `dist/`, and the local build cache. |

## Layout

```text
.
├── main.go                 # Entry point. Calls cmd.Execute.
├── cmd/root.go             # Cobra root command: flags, logging, mode choice.
├── app/blank.go            # Walks the arguments and runs the deletion.
├── tui/tui.go              # Bubble Tea model: state, sizing, layout.
├── tui/chrome.go           # The frame around the content: mark, panel, markers, palette.
├── utils/gen-dummy-df.py   # Generates a dummy tree for `just e2e`.
├── justfile                # Every task.
└── AGENTS.md               # Contributor guidelines.
```

The terminal interface follows the house terminal idiom: ANSI_Shadow block art, a `[ ]`
marker on the current row, square corners, and the Rose Pine palette. `tui/chrome.go` holds
the one border value and the one palette that every other renderer reads.
