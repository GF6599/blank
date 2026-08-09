package cmd

import (
	"bytes"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"blank/internal/testsupport"
)

func TestHeadlessCLIShredsFiles(t *testing.T) {
	testsupport.SetupStubCommands(t)
	t.Setenv("GOCACHE", filepath.Join(t.TempDir(), "gocache"))

	dataDir := t.TempDir()
	target := filepath.Join(dataDir, "secret.txt")
	if err := os.WriteFile(target, []byte("classified"), 0o600); err != nil {
		t.Fatalf("writing target file: %v", err)
	}

	repoRoot := moduleRoot(t)
	cmd := exec.Command("go", "run", ".", "--no-ui", dataDir)
	cmd.Dir = repoRoot
	var stdout bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stdout

	if err := cmd.Run(); err != nil {
		t.Fatalf("headless CLI failed: %v\nOutput:\n%s", err, stdout.String())
	}

	if out := stdout.String(); !strings.Contains(out, "Progress:") {
		t.Fatalf("expected progress output, got %s", out)
	}

	if _, err := os.Stat(target); !os.IsNotExist(err) {
		t.Fatalf("expected %s to be removed, err=%v", target, err)
	}

}

func moduleRoot(t *testing.T) string {
	t.Helper()
	_, file, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatalf("unable to determine caller path")
	}
	return filepath.Clean(filepath.Join(filepath.Dir(file), ".."))
}
