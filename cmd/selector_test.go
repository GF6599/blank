package cmd

import (
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// A run with no arguments asks for usage, refuses, or opens the selector.
// Given no terminal, then a terminal in a protected directory, then a terminal
// in an ordinary one, when each decides where to look for targets, then the
// first asks for usage, the second refuses and names the remedy, and only the
// third opens.
// Why it matters: the selector marks a row with one keystroke, so opening it in
// a directory the run already refuses to destroy puts the whole of it one key
// and one confirmation away.
func TestWhereARunWithNoArgumentsLooksForTargets(t *testing.T) {
	ordinary := t.TempDir()
	home, err := os.UserHomeDir()
	if err != nil {
		t.Fatalf("read the home directory: %v", err)
	}

	for _, tc := range []struct {
		name       string
		hasTTY     bool
		headless   bool
		dir        string
		wantUsage  bool
		wantRefuse bool
	}{
		{name: "a pipe on standard input", hasTTY: false, dir: ordinary, wantUsage: true},
		{name: "headless mode", hasTTY: true, headless: true, dir: ordinary, wantUsage: true},
		{name: "the home directory", hasTTY: true, dir: home, wantRefuse: true},
		{name: "the root of the filesystem", hasTTY: true, dir: "/", wantRefuse: true},
		{name: "an ordinary directory", hasTTY: true, dir: ordinary},
	} {
		t.Run(tc.name, func(t *testing.T) {
			// When: the run decides where to look.
			start, err := selectorStart(tc.hasTTY, tc.headless, tc.dir)

			switch {
			case tc.wantUsage:
				// Then: it asks for the usage rather than a selector.
				if !errors.Is(err, errShowUsage) {
					t.Fatalf("got (%q, %v), want the usage", start, err)
				}
			case tc.wantRefuse:
				// Then: it refuses, and says what to do instead.
				if err == nil {
					t.Fatalf("it opened a selector in %s", tc.dir)
				}
				if errors.Is(err, errShowUsage) {
					t.Fatalf("it asked for usage rather than refusing %s", tc.dir)
				}
				if !strings.Contains(err.Error(), "command line") {
					t.Fatalf("the refusal names no remedy: %v", err)
				}
			default:
				// Then: it opens on the directory it was given, which it may
				// name by the path every symlink on the way is resolved to.
				if err != nil {
					t.Fatalf("it refused an ordinary directory: %v", err)
				}
				if !sameDir(t, start, tc.dir) {
					t.Fatalf("it would open on %q, want %q", start, tc.dir)
				}
			}
		})
	}
}

func sameDir(t *testing.T, a, b string) bool {
	t.Helper()
	fa, err := os.Stat(a)
	if err != nil {
		t.Fatalf("stat %s: %v", a, err)
	}
	fb, err := os.Stat(b)
	if err != nil {
		t.Fatalf("stat %s: %v", b, err)
	}
	return os.SameFile(fa, fb)
}

// The refusal survives a directory reached under another name.
// Given a symlink whose target is the home directory, when the selector is
// asked to open through it, then it is refused.
// Why it matters: one directory has as many names as there are symlinks to it,
// so a guard that reads only the name it was handed is a guard with a door in
// it.
func TestTheSelectorRefusesAProtectedDirectoryUnderAnotherName(t *testing.T) {
	// Given: a symlink pointing at the home directory.
	home, err := os.UserHomeDir()
	if err != nil {
		t.Fatalf("read the home directory: %v", err)
	}
	link := filepath.Join(t.TempDir(), "shortcut")
	if err := os.Symlink(home, link); err != nil {
		t.Fatalf("make the symlink: %v", err)
	}

	// When: the selector is asked to open on the name that points at it.
	if _, err := selectorStart(true, false, link); err == nil {
		t.Fatalf("the selector opened on %s, which is %s", link, home)
	}
}
