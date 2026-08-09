# Repository Guidelines

## Project Structure and Module Organization

The entry point (`main.go`) wires the Cobra CLI in `cmd/`, the deletion engine lives in `app/`, and the Bubble Tea interface sits in `tui/`. Inside `tui/`, `tui.go` holds the model and the layout, and `chrome.go` holds the frame around the content: the mark, the panel, the markers, and the palette. Keep filesystem helpers and shared utilities in `utils/`, which currently holds the dummy data generator. Do not check in build output under `bin/` or `dist/`. Place a new package in a directory that mirrors its domain, and put each `_test.go` beside the code it covers.

## Build, Test, and Development Commands

- `just check` runs the full gate: format check, `go vet`, `staticcheck`, race tests, and build. Run it before every commit and read its exit code.
- `just fmt` rewrites the tree to be `gofmt`-clean. `just fmt-check` reports drift without rewriting.
- `just build` and `just build-race` produce `bin/blank`.
- `just install` builds and installs to `~/self-made-bin/blank`.
- `just e2e` generates a dummy tree with `utils/gen-dummy-df.py` and deletes it headlessly.
- `just info` prints the git version, commit, and build date. These are shell variables printed to stdout. Nothing is embedded in the binary, because `LDFLAGS` carries only `-s -w`.

## Coding Style and Naming Conventions

Use Go 1.24 idioms: exported symbols carry doc comments, indentation is tabs, and a short-lived goroutine helper may stay in the file that scopes it. CLI flags, commands, and Bubble Tea models use lowerCamelCase field names, as in `tui.model`. Prefer `log/slog` with the structured keys already used in `app/blank.go`. Avoid `fmt.Println` outside Cobra help text. Give a script or an asset a descriptive filename, as in `utils/gen-dummy-df.py`.

`tui/chrome.go` holds one border value and one palette. Every renderer reads them. Do not introduce a second copy of a colour or a border rune.

The mark is bare block art. The house terminal idiom calls for a braced `{name}` lockup, and this project drops the braces deliberately. Do not add them back during a sweep for consistency with another project. `TestMarkCarriesNoBraces` enforces it, and it covers both the block art and the wordmark that replaces it on a small terminal.

## Testing Guidelines

Tests use the Go toolchain alone, with no extra framework. Name a test `TestFeatureBehaviour` in a file ending `_test.go`. Use `t.TempDir()` and the stub executables in `internal/testsupport` to simulate `shred` and `rm` safely. Cover the success path and the error path.

Sweep a range of terminal sizes when you test the interface. A layout defect that under-budgets a row only overruns at the widths where the content nearly fits, so a test at three fixed sizes walks straight past it. `tui/chrome_test.go` shows the pattern.

Sweep the content as well as the size. A path measures one cell per rune only while it stays in ASCII, so a suite written in English proves nothing about the width maths. Include a wide-character case and an emoji case in any test that measures a row.

Run `just check` before submitting.

## Commit and Pull Request Guidelines

History uses gitmoji subjects in the form `<emoji> <scope>: <summary>`, for example `✨ tui: Add the framed activity panel`. Keep the subject under 50 characters and default to subject-only. Add a body only when the subject and the diff together cannot be understood.

A pull request states the motivation, lists the commands run, and links any tracking issue. Include a terminal capture when the change alters what the interface renders. Do not bundle build output. Call out any change to the deletion arguments explicitly, because those decide whether data is recoverable.
