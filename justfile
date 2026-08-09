set dotenv-load
set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

export GOCACHE := justfile_directory() / ".gocache"

BINARY := "blank"
BIN_DIR := "bin"
DIST_DIR := "dist"
CACHE_DIR := ".gocache"
INSTALL_DIR := home_directory() / "self-made-bin"

STATICCHECK := "honnef.co/go/tools/cmd/staticcheck@v0.7.0"

VERSION := `git describe --tags --always --dirty 2>/dev/null || echo dev`
COMMIT  := `git rev-parse --short HEAD 2>/dev/null || echo none`
DATE    := `date -u +%Y-%m-%dT%H:%M:%SZ`

LDFLAGS := "-s -w"
RELEASE_OS := "linux darwin"
RELEASE_ARCH := "amd64 arm64"

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

# Vet and lint go sources
[group('dev')]
lint:
	go vet ./...
	go run {{STATICCHECK}} ./...

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

# Generate a dummy tree and run end-to-end
[group('dev')]
e2e:
	tmp_dir="$(mktemp -d)"; \
	trap 'rm -rf "$tmp_dir"' EXIT; \
	python3 utils/gen-dummy-df.py --root "$tmp_dir" --force --many-files 10 --large-file-size 1 --depth 2 >/dev/null; \
	go run . --no-ui "$tmp_dir"

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

# Build one release target into dist/, e.g. `just release darwin arm64`
[group('deploy')]
release goos goarch:
	mkdir -p {{DIST_DIR}}
	CGO_ENABLED=0 GOOS={{goos}} GOARCH={{goarch}} go build -mod=readonly -trimpath \
		-ldflags "{{LDFLAGS}}" \
		-o {{DIST_DIR}}/{{BINARY}}-{{goos}}-{{goarch}}{{ if goos == "windows" { ".exe" } else { "" } }} .
	@echo "Built {{DIST_DIR}}/{{BINARY}}-{{goos}}-{{goarch}}{{ if goos == "windows" { ".exe" } else { "" } }}"

# Build every release target. CI calls this rather than repeating the matrix.
[group('deploy')]
release-all:
	for goos in {{RELEASE_OS}}; do \
		for goarch in {{RELEASE_ARCH}}; do \
			{{just_executable()}} release "$goos" "$goarch"; \
		done; \
	done

# Write SHA256SUMS over the release artifacts in dist/
[group('deploy')]
checksums:
	cd {{DIST_DIR}} && if command -v sha256sum >/dev/null; then \
		sha256sum {{BINARY}}-* > SHA256SUMS; \
	else \
		shasum -a 256 {{BINARY}}-* > SHA256SUMS; \
	fi
	@echo "Wrote {{DIST_DIR}}/SHA256SUMS"

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
