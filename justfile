set dotenv-load
set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

BINARY := "blank"
BIN_DIR := "bin"
DIST_DIR := "dist"
INSTALL_DIR := home_directory() / "self-made-bin"

# Show available recipes
default:
    just -l

# Run the app
[group('dev')]
run *args:
    cargo run -- {{ args }}

# Format the sources
[group('dev')]
fmt:
    cargo fmt

# Report sources that are not formatted, without rewriting them
[group('dev')]
fmt-check:
    cargo fmt --check

# Lint, and check the dependencies for advisories and licenses
[group('dev')]
lint:
    cargo clippy --all-targets --locked -- -D warnings
    cargo clippy --all-targets --locked --target x86_64-unknown-linux-gnu -- -D warnings
    cargo deny check

# Run the tests
[group('dev')]
test:
    cargo test --locked

# Run the full quality gate. CI and the commit commands call this by name.
[group('dev')]
check: fmt-check lint test
    cargo build --locked

# The tree holds a directory nobody can list, so a working run reports it,
# leaves it alone, and exits non-zero.

# Generate a dummy tree and run the build end to end
[group('dev')]
e2e:
    #!/usr/bin/env bash
    set -euo pipefail
    tmp_dir="$(mktemp -d)"; log_file="$(mktemp)"
    python3 utils/gen-dummy-df.py --root "$tmp_dir" --force --many-files 10 --large-file-size 1 --depth 2 >/dev/null
    status=0; cargo run --quiet -- --yes "$tmp_dir" >"$log_file" 2>&1 || status=$?
    verdict="the tree is gone, and the run named the directory it could not read"
    if [ "$status" -eq 0 ]; then verdict="exited 0 with a directory it cannot have read"
    elif ! grep -q no_list_dir "$log_file"; then verdict="did not name the directory it could not read"
    elif [ ! -d "$tmp_dir/no_list_dir" ]; then verdict="removed the directory it could not read"
    elif [ -e "$tmp_dir/no_read.txt" ]; then verdict="left the unreadable file on disk"; fi
    chmod -R u+rwX "$tmp_dir"; rm -rf "$tmp_dir"
    echo "e2e: $verdict"
    case "$verdict" in "the tree is gone"*) rm -f "$log_file" ;; *) cat "$log_file"; rm -f "$log_file"; exit 1 ;; esac

# Build the release binary into bin/
[group('deploy')]
build:
    cargo build --locked --release
    mkdir -p {{ BIN_DIR }}
    cp target/release/{{ BINARY }} {{ BIN_DIR }}/{{ BINARY }}
    @echo "Built {{ BIN_DIR }}/{{ BINARY }}"

# Build then install the binary into ~/self-made-bin
[group('deploy')]
install: build
    mkdir -p {{ INSTALL_DIR }}
    install -m 755 {{ BIN_DIR }}/{{ BINARY }} {{ INSTALL_DIR }}/{{ BINARY }}
    @command -v codesign >/dev/null && codesign -f -s - {{ INSTALL_DIR }}/{{ BINARY }} || true
    @echo "Installed {{ BINARY }} to {{ INSTALL_DIR }}/{{ BINARY }}"

# Render a full release into dist/ and publish none of it
[group('deploy')]
release-dry-run:
    goreleaser release --snapshot --clean --skip=publish

# Remove build output
[group('deploy')]
clean:
    rm -rf {{ BIN_DIR }} {{ DIST_DIR }}
    cargo clean
