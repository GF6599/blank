# Repository Guidelines

## Project Structure & Module Organization

The entry point (`main.go`) wires Cobra’s CLI in `cmd/`, the shredding engine lives in `app/`, and Bubble Tea UI layers sit in `tui/`. Keep filesystem helpers and future shared utilities in `utils/` (currently hosts tooling for dummy data). Generated binaries such as `gs/` or artifacts under `bin/` / `dist/` should not be checked in. Place new packages under a directory that mirrors their domain, and co-locate `_test.go` files beside the code they cover.

## Build, Test, and Development Commands

- `just fmt` ensures the repo stays `gofmt`-clean.
- `just build` (or `just build-race`) resolves deps, formats, and produces `bin/gshred`.
- `go run . ~/Downloads/secret.txt` is the quickest local sanity check.
- `go test ./...` should pass before every push; add `-race` for concurrency-heavy changes.
- `just info` prints the version, commit, and build timestamp embedded via ldflags.

## Coding Style & Naming Conventions

Use Go 1.24 idioms: exported symbols require doc comments, indentation is tabs, and short-lived goroutine helpers can stay in the same file when scoped. CLI flags, commands, and Bubble Tea models follow lowerCamelCase field names (see `tui/model`). Prefer `log/slog` with structured keys already used in `app/gshred.go`; avoid `fmt.Println` except for Cobra help text. Scripts and assets should favor descriptive filenames like `utils/gen-dummy-df.py`.

## Testing Guidelines

Unit tests rely on the Go toolchain—no extra frameworks. Name tests `TestFeatureBehavior` in files ending with `_test.go`. Use `t.TempDir()` plus stub executables on `PATH` to safely simulate `shred` and `rm -rf`, and cover both success paths and error logging. Target meaningful coverage for concurrency primitives: validate progress channel closure, semaphore limits, and symlink handling. Run `go test ./app ./tui` (optionally with `-race`) before submitting.

## Commit & Pull Request Guidelines

History shows short, imperative subjects (“Add justfile”, “Update dependencies”); keep using that format and limit to ~72 characters. Every pull request should summarize motivation, list test commands run (`just build`, `go test ./...`), and link any tracking issues. Include screenshots or terminal captures when changing TUI output or logging. Avoid bundling generated binaries; mention any security-impacting changes (e.g., tweaks to `shred` arguments) explicitly.
