set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

BINARY := "gshred"
BIN_DIR := "bin"
DIST_DIR := "dist"

VERSION := `git describe --tags --always --dirty 2>/dev/null || echo dev`
COMMIT  := `git rev-parse --short HEAD 2>/dev/null || echo none`
DATE    := `date -u +%Y-%m-%dT%H:%M:%SZ`

LDFLAGS := "-s -w"

default:
  just -l

fmt:
	go fmt ./...

run *args:
	GOCACHE="$(pwd)/.gocache" go run . {{args}}

test:
	GOCACHE="$(pwd)/.gocache" go test ./...

e2e:
	tmp_dir="$(mktemp -d)"; \
	GOCACHE="$(pwd)/.gocache" python3 utils/gen-dummy-df.py --root "$tmp_dir" --force --many-files 10 --large-file-size 1 --depth 2 >/dev/null; \
	GOCACHE="$(pwd)/.gocache" go run . --no-ui "$tmp_dir"; \
	rm -rf "$tmp_dir"

deps:
	go mod tidy
	go mod verify

build: deps fmt
	mkdir -p {{BIN_DIR}}
	go build -mod=readonly -trimpath -ldflags "{{LDFLAGS}}" -o {{BIN_DIR}}/{{BINARY}} .
	@echo "Built {{BIN_DIR}}/{{BINARY}} (version {{VERSION}} commit {{COMMIT}} date {{DATE}})"

build-race: deps fmt
	mkdir -p {{BIN_DIR}}
	go build -mod=readonly -trimpath -race -ldflags "{{LDFLAGS}}" -o {{BIN_DIR}}/{{BINARY}}-race .
	@echo "Built {{BIN_DIR}}/{{BINARY}}-race (version {{VERSION}} commit {{COMMIT}})"

release: deps fmt
	mkdir -p {{DIST_DIR}}
	CGO_ENABLED=0 GOOS=linux GOARCH=amd64 go build -mod=readonly -trimpath -ldflags "{{LDFLAGS}}" -o {{DIST_DIR}}/{{BINARY}}-linux-amd64 .
	@echo "Built {{DIST_DIR}}/{{BINARY}}-linux-amd64 (version {{VERSION}} commit {{COMMIT}})"

clean:
	rm -rf {{BIN_DIR}} {{DIST_DIR}}

info:
	@echo VERSION={{VERSION}}\nCOMMIT={{COMMIT}}\nDATE={{DATE}}
