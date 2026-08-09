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

// A dry run names every target and destroys none of them.
// Given a tree of files, when it is dry run, then each target is listed and the
// whole tree is still on disk.
// Why it matters: this is the only way to check the aim of a command whose
// mistakes cannot be undone, so a dry run that touched anything, or that left a
// target out of the list, would be worse than not having one.
func TestDryRunNamesEveryTargetAndDestroysNothing(t *testing.T) {
	// Given: a tree holding a file, a symlink, and a nested directory.
	root := t.TempDir()
	nested := filepath.Join(root, "nested")
	if err := os.MkdirAll(nested, 0o700); err != nil {
		t.Fatalf("creating the nested directory: %v", err)
	}
	note := filepath.Join(nested, "note.txt")
	if err := os.WriteFile(note, []byte("payload"), 0o600); err != nil {
		t.Fatalf("writing the note: %v", err)
	}
	link := filepath.Join(root, "link")
	if err := os.Symlink(note, link); err != nil {
		t.Fatalf("creating the symlink: %v", err)
	}

	// When: the tree is dry run. No terminal is attached, so reaching the end at
	// all is also the proof the interface was never started.
	out, code := run(t, "--dry-run", root)

	// Then: it exits clean.
	if code != 0 {
		t.Fatalf("a dry run over a readable tree exited %d\n%s", code, out)
	}
	// And: every target is named.
	for _, path := range []string{note, link, nested, root} {
		if !strings.Contains(out, path) {
			t.Fatalf("the dry run does not name %s\n%s", path, out)
		}
	}
	// And: the tree is untouched.
	body, err := os.ReadFile(note)
	if err != nil || string(body) != "payload" {
		t.Fatalf("the dry run destroyed the file: contents are %q, err=%v", body, err)
	}
	if _, err := os.Lstat(link); err != nil {
		t.Fatalf("the dry run removed the symlink: %v", err)
	}
}

// A dry run that could not read everything says so in its exit code.
// Given a directory the walk cannot list, when the tree is dry run, then it
// exits non-zero and names that directory.
// Why it matters: the plan is a promise about what the run would destroy, and a
// plan built over a directory nobody could read is missing whatever is inside
// it. Reporting that as a clean plan would understate the blast radius.
func TestDryRunReportsWhatItCouldNotRead(t *testing.T) {
	if os.Geteuid() == 0 {
		t.Skip("root ignores directory permissions, so the listing cannot fail")
	}

	// Given: a tree holding a directory nobody can read.
	root := t.TempDir()
	locked := filepath.Join(root, "locked")
	if err := os.MkdirAll(locked, 0o700); err != nil {
		t.Fatalf("creating the locked directory: %v", err)
	}
	if err := os.Chmod(locked, 0o000); err != nil {
		t.Fatalf("locking the directory: %v", err)
	}
	t.Cleanup(func() { _ = os.Chmod(locked, 0o700) })

	// When: the tree is dry run.
	out, code := run(t, "--dry-run", root)

	// Then: it exits non-zero, naming the directory it could not read.
	if code == 0 {
		t.Fatalf("a dry run over an unreadable directory exited 0\n%s", out)
	}
	if !strings.Contains(out, locked) {
		t.Fatalf("the dry run does not name %s\n%s", locked, out)
	}
}

func moduleRoot() string {
	_, file, _, ok := runtime.Caller(0)
	if !ok {
		panic("unable to determine caller path")
	}
	return filepath.Clean(filepath.Join(filepath.Dir(file), ".."))
}
