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

# Format go sources
[group('dev')]
fmt:
	go fmt ./...

# Report sources that are not gofmt-clean, without rewriting them
[group('dev')]
fmt-check:
	@test -z "$(gofmt -l .)" || { echo "Not gofmt-clean:"; gofmt -l .; echo "Run 'just fmt'."; exit 1; }

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

# Run the tests
[group('dev')]
test:
	go test ./...

# Run the tests with the race detector
[group('dev')]
test-race:
	go test -race -count=1 ./...

# Run the full quality gate. CI and the commit commands call this by name.
[group('dev')]
check: fmt-check lint test-race
	go build ./...

# The tree holds a directory nobody can list, so a working run reports it,
# leaves it alone, and exits non-zero.

# Generate a dummy tree and run end to end
[group('dev')]
e2e:
	tmp_dir="$(mktemp -d)"; log_file="$(mktemp)"; \
	trap 'chmod -R u+rwX "$tmp_dir" 2>/dev/null || true; rm -rf "$tmp_dir" "$log_file"' EXIT; \
	python3 utils/gen-dummy-df.py --root "$tmp_dir" --force --many-files 10 --large-file-size 1 --depth 2 >/dev/null; \
	if go run . --no-ui --yes "$tmp_dir" >"$log_file" 2>&1; then \
		cat "$log_file"; echo "e2e: exited 0 with a directory it cannot have read"; exit 1; \
	fi; \
	grep -q no_list_dir "$log_file" || { cat "$log_file"; echo "e2e: did not name the directory it could not read"; exit 1; }; \
	test -d "$tmp_dir/no_list_dir" || { echo "e2e: removed the directory it could not read"; exit 1; }; \
	test ! -e "$tmp_dir/no_read.txt" || { echo "e2e: left the unreadable file on disk"; exit 1; }; \
	echo "e2e: the tree is gone, and the run named the directory it could not read"

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

# Remove build output and the local go cache
[group('deploy')]
clean:
	rm -rf {{BIN_DIR}} {{DIST_DIR}} {{CACHE_DIR}}

# Print the git version, commit, and build date
[group('deploy')]
info:
	@echo "VERSION={{VERSION}}"
	@echo "COMMIT={{COMMIT}}"
	@echo "DATE={{DATE}}"
