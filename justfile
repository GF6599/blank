set dotenv-load
set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

export GOCACHE := justfile_directory() / ".gocache"

BINARY := "gshred"
BIN_DIR := "bin"
DIST_DIR := "dist"

VERSION := `git describe --tags --always --dirty 2>/dev/null || echo dev`
COMMIT  := `git rev-parse --short HEAD 2>/dev/null || echo none`
DATE    := `date -u +%Y-%m-%dT%H:%M:%SZ`

LDFLAGS := "-s -w"

# Show available recipes
default:
  just -l

# Format go sources
[group('dev')]
fmt:
	go fmt ./...

# Run the app
[group('run')]
run *args:
	go run . {{args}}

# Run the tests
[group('dev')]
test:
	go test ./...

# Generate a dummy tree and run end-to-end
[group('dev')]
e2e:
	tmp_dir="$(mktemp -d)"; \
	python3 utils/gen-dummy-df.py --root "$tmp_dir" --force --many-files 10 --large-file-size 1 --depth 2 >/dev/null; \
	go run . --no-ui "$tmp_dir"; \
	rm -rf "$tmp_dir"

# Tidy and verify go modules
[group('build')]
deps:
	go mod tidy
	go mod verify

# Build the binary into bin/
[group('build')]
build: deps fmt
	mkdir -p {{BIN_DIR}}
	go build -mod=readonly -trimpath -ldflags "{{LDFLAGS}}" -o {{BIN_DIR}}/{{BINARY}} .
	@echo "Built {{BIN_DIR}}/{{BINARY}} (version {{VERSION}} commit {{COMMIT}} date {{DATE}})"

# Build the race-enabled binary into bin/
[group('build')]
build-race: deps fmt
	mkdir -p {{BIN_DIR}}
	go build -mod=readonly -trimpath -race -ldflags "{{LDFLAGS}}" -o {{BIN_DIR}}/{{BINARY}}-race .
	@echo "Built {{BIN_DIR}}/{{BINARY}}-race (version {{VERSION}} commit {{COMMIT}})"

# Build the linux amd64 binary into bin/
[group('build')]
build-linux: deps fmt
	mkdir -p {{BIN_DIR}}
	GOOS=linux GOARCH=amd64 go build -mod=readonly -trimpath -ldflags "{{LDFLAGS}}" -o {{BIN_DIR}}/{{BINARY}}-linux .
	@echo "Built {{BIN_DIR}}/{{BINARY}}-race (version {{VERSION}} commit {{COMMIT}})"

# Build the linux amd64 release binary into dist/
[group('build')]
release: deps fmt
	mkdir -p {{DIST_DIR}}
	CGO_ENABLED=0 GOOS=linux GOARCH=amd64 go build -mod=readonly -trimpath -ldflags "{{LDFLAGS}}" -o {{DIST_DIR}}/{{BINARY}}-linux-amd64 .
	@echo "Built {{DIST_DIR}}/{{BINARY}}-linux-amd64 (version {{VERSION}} commit {{COMMIT}})"

# Remove bin/ and dist/
[group('build')]
clean:
	rm -rf {{BIN_DIR}} {{DIST_DIR}}

# Print version, commit, and build date
[group('build')]
info:
	@echo VERSION={{VERSION}}\nCOMMIT={{COMMIT}}\nDATE={{DATE}}
