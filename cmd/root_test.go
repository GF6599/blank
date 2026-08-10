package cmd

import (
	"bytes"
	"context"
	"io"
	"log/slog"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"blank/app"
)

func TestHeadlessRunUsesTheGivenWriter(t *testing.T) {
	path := filepath.Join(t.TempDir(), "target")
	if err := os.WriteFile(path, []byte("payload"), 0o600); err != nil {
		t.Fatalf("writing target: %v", err)
	}
	plan, err := app.Enumerate([]string{path})
	if err != nil {
		t.Fatalf("enumerating target: %v", err)
	}
	var out bytes.Buffer
	if err := runHeadless(context.Background(), &out, plan, app.Options{Jobs: 1, Passes: 0}); err != nil {
		t.Fatalf("running headlessly: %v", err)
	}
	if !strings.Contains(out.String(), "Activity:") || !strings.Contains(out.String(), "Progress: 100%") {
		t.Fatalf("the supplied writer received:\n%s", out.String())
	}
}

func TestHeadlessRunHonoursACancelledContext(t *testing.T) {
	path := filepath.Join(t.TempDir(), "target")
	if err := os.WriteFile(path, []byte("payload"), 0o600); err != nil {
		t.Fatalf("writing target: %v", err)
	}
	plan, err := app.Enumerate([]string{path})
	if err != nil {
		t.Fatalf("enumerating target: %v", err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if err := runHeadless(ctx, &bytes.Buffer{}, plan, app.Options{Jobs: 1, Passes: 0}); err == nil {
		t.Fatal("a cancelled headless run reported success")
	}
	if _, err := os.Lstat(path); err != nil {
		t.Fatalf("the cancelled target did not survive: %v", err)
	}
}

func TestQuietLoggerRestoresTheDefaultLogger(t *testing.T) {
	before := slog.Default()
	restore := quietLogger()
	if slog.Default() == before {
		t.Fatal("logging was not suppressed")
	}
	restore()
	if slog.Default() != before {
		t.Fatal("the default logger leaked past the command")
	}
}

// The enumeration never prints over the interface it is about to hand the
// terminal to.
// Given a directory the walk cannot list, when the command runs without --no-ui,
// then nothing the walk logged reaches the default logger.
// Why it matters: the interface draws inline rather than on its own screen, so a
// line printed on the way past it lands in the middle of what the operator reads
// before deciding whether to let the run continue.
func TestTheEnumerationStaysQuietWhileTheInterfaceOwnsTheTerminal(t *testing.T) {
	if os.Geteuid() == 0 {
		t.Skip("root ignores directory permissions, so the listing cannot fail")
	}
	// Given: a directory the walk cannot list.
	root := t.TempDir()
	locked := filepath.Join(root, "locked")
	if err := os.Mkdir(locked, 0o000); err != nil {
		t.Fatalf("creating the locked directory: %v", err)
	}
	t.Cleanup(func() { _ = os.Chmod(locked, 0o700) })

	var logged bytes.Buffer
	previous := slog.Default()
	slog.SetDefault(slog.New(slog.NewTextHandler(&logged, &slog.HandlerOptions{})))
	t.Cleanup(func() { slog.SetDefault(previous) })

	// When: the command runs with the interface asked for.
	command := NewRootCommand()
	command.SetArgs([]string{"--yes", root})
	command.SetOut(io.Discard)
	command.SetErr(io.Discard)
	_ = command.Execute()

	// Then: the walk printed nothing to the terminal.
	if logged.Len() != 0 {
		t.Fatalf("the enumeration printed over the interface:\n%s", logged.String())
	}
}

func TestRootCommandsDoNotShareFlagValues(t *testing.T) {
	first := NewRootCommand()
	if err := first.Flags().Set("jobs", "1"); err != nil {
		t.Fatalf("setting first command: %v", err)
	}
	second := NewRootCommand()
	jobs, err := second.Flags().GetInt("jobs")
	if err != nil {
		t.Fatalf("reading second command: %v", err)
	}
	if jobs != app.DefaultJobs {
		t.Fatalf("the second command inherited jobs=%d", jobs)
	}
}
