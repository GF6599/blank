package testsupport

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
)

// SetupStubCommands prepends fake shred/rm binaries to PATH that log activity
// and delegate to the system rm so tests can run without touching real data.
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
if [ -n "$target" ] && [ -e "$target" ]; then
	"$REAL_RM" -f "$target"
fi
`, logFile, realRm))

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
