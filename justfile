set dotenv-load
set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

export GOCACHE := justfile_directory() / ".gocache"

BINARY := "blank"
BIN_DIR := "bin"
DIST_DIR := "dist"
CACHE_DIR := ".gocache"
INSTALL_DIR := home_directory() / "self-made-bin"

VERSION := `git describe --tags --always --dirty 2>/dev/null || echo dev`
COMMIT  := `git rev-parse --short HEAD 2>/dev/null || echo none`
DATE    := `date -u +%Y-%m-%dT%H:%M:%SZ`

LDFLAGS := "-s -w"

# Show available recipes
default:
  just -l

# Run the app
[group('dev')]
run *args:
	go run . {{args}}

# Format the Go and Rust sources
[group('dev')]
fmt:
	go fmt ./...
	cargo fmt

# Report sources that are not formatted, without rewriting them
[group('dev')]
fmt-check:
	@test -z "$(gofmt -l .)" || { echo "Not gofmt-clean:"; gofmt -l .; echo "Run 'just fmt'."; exit 1; }
	cargo fmt --check

# Every tool is pinned by a tool directive in go.mod, so `go tool` builds it
# against the module's own dependency graph. `go run <pkg>@<version>` resolved
# x/tools through the tool's go.mod instead, which pinned a version too old to
# decode the export data a current toolchain emits.

# Vet, lint, and check the dependencies for known vulnerabilities
[group('dev')]
lint:
	go vet ./...
	go tool staticcheck ./...
	go tool govulncheck ./...
	cargo clippy --all-targets --locked -- -D warnings
	cargo clippy --all-targets --locked --target x86_64-unknown-linux-gnu -- -D warnings
	cargo deny check

# Run the tests
[group('dev')]
test: _test-rust
	go test ./...

# Run the tests, with the race detector on the Go side
[group('dev')]
test-race: _test-rust
	go test -race -count=1 ./...

# Run the Rust tests
[private]
_test-rust:
	cargo test --locked

# Run the full quality gate. CI and the commit commands call this by name.
[group('dev')]
check: fmt-check lint test-race
	go build ./...
	cargo build --locked

# The tree holds a directory nobody can list, so a working run reports it,
# leaves it alone, and exits non-zero. Each build gets a fresh tree while the
# Go build is still the reference.

# Generate a dummy tree and run each build end to end
[group('dev')]
e2e:
	#!/usr/bin/env bash
	set -euo pipefail
	for build in "go run . --no-ui" "cargo run --quiet --"; do
		tmp_dir="$(mktemp -d)"; log_file="$(mktemp)"
		python3 utils/gen-dummy-df.py --root "$tmp_dir" --force --many-files 10 --large-file-size 1 --depth 2 >/dev/null
		status=0; $build --yes "$tmp_dir" >"$log_file" 2>&1 || status=$?
		verdict="the tree is gone, and the run named the directory it could not read"
		if [ "$status" -eq 0 ]; then verdict="exited 0 with a directory it cannot have read"
		elif ! grep -q no_list_dir "$log_file"; then verdict="did not name the directory it could not read"
		elif [ ! -d "$tmp_dir/no_list_dir" ]; then verdict="removed the directory it could not read"
		elif [ -e "$tmp_dir/no_read.txt" ]; then verdict="left the unreadable file on disk"; fi
		chmod -R u+rwX "$tmp_dir"; rm -rf "$tmp_dir"
		echo "e2e ($build): $verdict"
		case "$verdict" in "the tree is gone"*) rm -f "$log_file" ;; *) cat "$log_file"; rm -f "$log_file"; exit 1 ;; esac
	done

# Tidy and verify go modules. Rewrites go.mod, so no other recipe depends on it.
[group('dev')]
deps:
	go mod tidy
	go mod verify

# Build the binary into bin/
[group('deploy')]
build:
	mkdir -p {{BIN_DIR}}
	go build -mod=readonly -trimpath -ldflags "{{LDFLAGS}}" -o {{BIN_DIR}}/{{BINARY}} .
	@echo "Built {{BIN_DIR}}/{{BINARY}}"

# Build the race-enabled binary into bin/
[group('deploy')]
build-race:
	mkdir -p {{BIN_DIR}}
	go build -mod=readonly -trimpath -race -ldflags "{{LDFLAGS}}" -o {{BIN_DIR}}/{{BINARY}}-race .
	@echo "Built {{BIN_DIR}}/{{BINARY}}-race"

# Build then install the binary into ~/self-made-bin
[group('deploy')]
install: build
	mkdir -p {{INSTALL_DIR}}
	install -m 755 {{BIN_DIR}}/{{BINARY}} {{INSTALL_DIR}}/{{BINARY}}
	@command -v codesign >/dev/null && codesign -f -s - {{INSTALL_DIR}}/{{BINARY}} || true
	@echo "Installed {{BINARY}} to {{INSTALL_DIR}}/{{BINARY}}"

# Render a full release into dist/ and publish none of it
[group('deploy')]
release-dry-run:
	goreleaser release --snapshot --clean --skip=publish

# Remove build output and the local caches
[group('deploy')]
clean:
	rm -rf {{BIN_DIR}} {{DIST_DIR}} {{CACHE_DIR}}
	cargo clean

# Print the git version, commit, and build date
[group('deploy')]
info:
	@echo "VERSION={{VERSION}}"
	@echo "COMMIT={{COMMIT}}"
	@echo "DATE={{DATE}}"
