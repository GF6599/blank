package cmd

import (
	"bytes"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

// blankBinary is the command under test, built once for the whole package.
var blankBinary string

// TestMain builds the binary rather than letting each test run `go run`, which
// compiles the module again every time it is called.
func TestMain(m *testing.M) {
	dir, err := os.MkdirTemp("", "blank-e2e")
	if err != nil {
		fmt.Fprintf(os.Stderr, "creating the build directory: %v\n", err)
		os.Exit(1)
	}
	blankBinary = filepath.Join(dir, "blank")
	build := exec.Command("go", "build", "-o", blankBinary, ".")
	build.Dir = moduleRoot()
	if out, err := build.CombinedOutput(); err != nil {
		fmt.Fprintf(os.Stderr, "building the binary: %v\n%s", err, out)
		os.RemoveAll(dir)
		os.Exit(1)
	}
	code := m.Run()
	os.RemoveAll(dir)
	os.Exit(code)
}

// run invokes the built binary and returns everything it wrote and its exit
// code. Both streams are joined, because a caller reads them joined.
func run(t *testing.T, args ...string) (string, int) {
	t.Helper()
	cmd := exec.Command(blankBinary, args...)
	var out bytes.Buffer
	cmd.Stdout = &out
	cmd.Stderr = &out
	err := cmd.Run()
	var exit *exec.ExitError
	switch {
	case err == nil:
		return out.String(), 0
	case errors.As(err, &exit):
		return out.String(), exit.ExitCode()
	default:
		t.Fatalf("running the binary: %v\n%s", err, out.String())
		return "", 0
	}
}

func TestHeadlessCLIShredsFiles(t *testing.T) {
	dataDir := t.TempDir()
	target := filepath.Join(dataDir, "secret.txt")
	if err := os.WriteFile(target, []byte("classified"), 0o600); err != nil {
		t.Fatalf("writing target file: %v", err)
	}

	out, code := run(t, "--no-ui", dataDir)
	if code != 0 {
		t.Fatalf("headless CLI exited %d\nOutput:\n%s", code, out)
	}
	if !strings.Contains(out, "Progress:") {
		t.Fatalf("expected progress output, got %s", out)
	}
	if _, err := os.Stat(target); !os.IsNotExist(err) {
		t.Fatalf("expected %s to be removed, err=%v", target, err)
	}
}

func moduleRoot() string {
	_, file, _, ok := runtime.Caller(0)
	if !ok {
		panic("unable to determine caller path")
	}
	return filepath.Clean(filepath.Join(filepath.Dir(file), ".."))
}
