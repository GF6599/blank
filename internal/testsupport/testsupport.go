package testsupport

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
)

// FailMarker names the environment variable the shred stub reads. A target whose
// path contains the value fails, so a test can drive the error path without
// needing a file the real shred cannot touch.
const FailMarker = "BLANK_STUB_FAIL"

// SetupStubCommands prepends fake shred and rm binaries to PATH that log every
// call and delegate to the system rm, so tests never touch real data.
func SetupStubCommands(t *testing.T) string {
	t.Helper()
	stubDir := t.TempDir()
	logFile := filepath.Join(stubDir, "cmd.log")

	realRm, err := exec.LookPath("rm")
	if err != nil {
		t.Fatalf("unable to find rm binary: %v", err)
	}

	writeExecutable(t, filepath.Join(stubDir, "shred"), fmt.Sprintf(`#!/bin/sh
set -eu
log=%q
REAL_RM=%q
target=""
for arg in "$@"; do
	target="$arg"
done
printf '%%s %%s\n' "shred" "$target" >> "$log"
if [ -n "${%s:-}" ] && [ "${target#*"${%s}"}" != "$target" ]; then
	echo "shred: $target: simulated failure" >&2
	exit 1
fi
if [ -n "$target" ] && [ -e "$target" ]; then
	"$REAL_RM" -f "$target"
fi
`, logFile, realRm, FailMarker, FailMarker))

	writeExecutable(t, filepath.Join(stubDir, "rm"), fmt.Sprintf(`#!/bin/sh
set -eu
log=%q
REAL_RM=%q
printf '%%s %%s\n' "rm" "$*" >> "$log"
exec "$REAL_RM" "$@"
`, logFile, realRm))

	oldPath := os.Getenv("PATH")
	newPath := stubDir + string(os.PathListSeparator) + oldPath
	t.Setenv("PATH", newPath)

	return logFile
}

func writeExecutable(t *testing.T, path, content string) {
	t.Helper()
	if err := os.WriteFile(path, []byte(content), 0o755); err != nil {
		t.Fatalf("failed to write stub %s: %v", path, err)
	}
}
