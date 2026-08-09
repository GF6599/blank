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
// code. Both streams are joined, because a caller reads them joined. Nothing is
// on standard input, which is what a command in a pipeline sees.
func run(t *testing.T, args ...string) (string, int) {
	t.Helper()
	return runWithInput(t, "", args...)
}

func runWithInput(t *testing.T, stdin string, args ...string) (string, int) {
	t.Helper()
	cmd := exec.Command(blankBinary, args...)
	cmd.Stdin = strings.NewReader(stdin)
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

// The help names the mode a script reads.
// Given the help text, when it renders, then --no-ui is in it.
// Why it matters: --no-ui is the only mode that prints one plain line per
// target, and a hidden flag is one a caller can find only by reading the
// source of the tool they are scripting.
func TestHelpNamesTheHeadlessMode(t *testing.T) {
	// When: the help renders.
	out, code := run(t, "--help")

	// Then: it exits clean.
	if code != 0 {
		t.Fatalf("--help exited %d:\n%s", code, out)
	}
	// And: it names the flag.
	if !strings.Contains(out, "--no-ui") {
		t.Fatalf("the help does not name --no-ui:\n%s", out)
	}
}

func TestHeadlessCLIShredsFiles(t *testing.T) {
	dataDir := t.TempDir()
	target := filepath.Join(dataDir, "secret.txt")
	if err := os.WriteFile(target, []byte("classified"), 0o600); err != nil {
		t.Fatalf("writing target file: %v", err)
	}

	out, code := run(t, "--no-ui", "--yes", dataDir)
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

// Nothing is destroyed until the operator answers the question with a whole yes.
// Given a tree and an answer on standard input, when the run is asked to start,
// then only a whole "yes" lets it destroy anything.
// Why it matters: the command destroys beyond recovery and its arguments are
// shell globs, so the last check on a mistyped target is the operator reading
// back what is about to go. A closed input has to read as a refusal, or a
// command in a pipeline destroys a tree nobody ever confirmed.
func TestNothingIsDestroyedUntilTheOperatorAnswersYes(t *testing.T) {
	for name, answer := range map[string]struct {
		typed     string
		destroyed bool
	}{
		"a whole yes":         {"yes\n", true},
		"a yes in capitals":   {"YES\n", true},
		"a bare y":            {"y\n", false},
		"a no":                {"no\n", false},
		"an empty line":       {"\n", false},
		"no answer at all":    {"", false},
		"a yes with padding":  {"  yes  \n", true},
		"an answer of yes-no": {"yesno\n", false},
	} {
		t.Run(name, func(t *testing.T) {
			// Given: a tree with something to lose.
			root := t.TempDir()
			victim := filepath.Join(root, "victim.txt")
			if err := os.WriteFile(victim, []byte("payload"), 0o600); err != nil {
				t.Fatalf("writing the victim: %v", err)
			}

			// When: the run is given that answer.
			out, code := runWithInput(t, answer.typed, "--no-ui", root)

			// Then: the tree is gone only when the answer was a whole yes.
			_, err := os.Stat(victim)
			if gone := os.IsNotExist(err); gone != answer.destroyed {
				t.Fatalf("the file was destroyed=%v, expected %v\n%s", gone, answer.destroyed, out)
			}
			// And: a run that destroyed nothing does not exit as if it had.
			if !answer.destroyed && code == 0 {
				t.Fatalf("a refused run exited 0\n%s", out)
			}
			if answer.destroyed && code != 0 {
				t.Fatalf("a confirmed run exited %d\n%s", code, out)
			}
		})
	}
}

// The question names what is about to go.
// Given a tree of known shape, when the run asks, then the question counts the
// files, the removals, and the directories.
// Why it matters: a bare "are you sure" is answered yes by reflex. The counts
// are what make a command aimed at the wrong directory visible in time.
func TestThePromptCountsWhatIsAboutToBeDestroyed(t *testing.T) {
	// Given: a tree holding two files, one symlink, and two directories.
	root := t.TempDir()
	nested := filepath.Join(root, "nested")
	if err := os.MkdirAll(nested, 0o700); err != nil {
		t.Fatalf("creating the nested directory: %v", err)
	}
	for _, name := range []string{"one.txt", "nested/two.txt"} {
		if err := os.WriteFile(filepath.Join(root, name), []byte("payload"), 0o600); err != nil {
			t.Fatalf("writing %s: %v", name, err)
		}
	}
	if err := os.Symlink(filepath.Join(root, "one.txt"), filepath.Join(root, "link")); err != nil {
		t.Fatalf("creating the symlink: %v", err)
	}

	// When: the run asks and is refused.
	out, _ := runWithInput(t, "no\n", "--no-ui", root)

	// Then: the question counts each kind.
	for _, want := range []string{"2 files", "1 symlink or special file", "2 directories"} {
		if !strings.Contains(out, want) {
			t.Fatalf("the question does not say %q\n%s", want, out)
		}
	}
}

// A scripted run skips the question rather than hanging on it.
// Given no answer on standard input, when the run is given --yes, then the tree
// is destroyed.
// Why it matters: the prompt would otherwise make the command unusable from a
// script or a cron job, which is the pressure that gets safety checks removed.
func TestYesSkipsTheQuestion(t *testing.T) {
	// Given: a tree with something to lose, and nothing on standard input.
	root := t.TempDir()
	victim := filepath.Join(root, "victim.txt")
	if err := os.WriteFile(victim, []byte("payload"), 0o600); err != nil {
		t.Fatalf("writing the victim: %v", err)
	}

	// When: the run is given --yes.
	out, code := run(t, "--no-ui", "--yes", root)

	// Then: it destroyed the tree and exited clean.
	if code != 0 {
		t.Fatalf("a run with --yes exited %d\n%s", code, out)
	}
	if _, err := os.Stat(victim); !os.IsNotExist(err) {
		t.Fatalf("expected %s to be destroyed, err=%v", victim, err)
	}
}

// The log is written where the caller asked for it.
// Given a path for the log, when a tree is destroyed, then the log is at that
// path and names the file it destroyed.
// Why it matters: the log is a manifest of what was destroyed and where it
// lived, so writing it beside whoever ran the command leaves that record in a
// directory they did not choose, and often did not expect.
func TestTheLogLandsWhereItWasAsked(t *testing.T) {
	// Given: a tree, and a path for the log well away from it.
	root := t.TempDir()
	secret := filepath.Join(root, "secret.txt")
	if err := os.WriteFile(secret, []byte("payload"), 0o600); err != nil {
		t.Fatalf("writing the file: %v", err)
	}
	manifest := filepath.Join(t.TempDir(), "manifest.log")

	// When: the tree is destroyed with the log turned on.
	out, code := run(t, "--no-ui", "--yes", "--log", manifest, root)
	if code != 0 {
		t.Fatalf("the run exited %d\n%s", code, out)
	}

	// Then: the log is at the path given, naming the file it destroyed.
	body, err := os.ReadFile(manifest)
	if err != nil {
		t.Fatalf("reading the log: %v", err)
	}
	if !strings.Contains(string(body), secret) {
		t.Fatalf("the log does not name %s:\n%s", secret, body)
	}
}

// The log is readable only by the user who ran the command.
// Given logging turned on, when the log is written, then no group or other
// permission bit is set on it.
// Why it matters: the log records the full path of every file the run touched,
// so it is a manifest of what was destroyed and where it lived.
func TestTheLogIsReadableOnlyByItsOwner(t *testing.T) {
	// Given: a tree and a path for the log.
	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, "secret.txt"), []byte("payload"), 0o600); err != nil {
		t.Fatalf("writing the file: %v", err)
	}
	manifest := filepath.Join(t.TempDir(), "manifest.log")

	// When: the run writes it.
	if out, code := run(t, "--no-ui", "--yes", "--log", manifest, root); code != 0 {
		t.Fatalf("the run exited %d\n%s", code, out)
	}

	// Then: the file carries no group or other permission.
	fi, err := os.Stat(manifest)
	if err != nil {
		t.Fatalf("expected a log file: %v", err)
	}
	if perm := fi.Mode().Perm(); perm&0o077 != 0 {
		t.Fatalf("the log is mode %04o, expected no group or other access", perm)
	}
}

// A log that cannot be opened stops the run before it destroys anything.
// Given a log path in a directory that does not exist, when the run is asked to
// start, then it exits non-zero and the tree is untouched.
// Why it matters: the log is the only record of what a run destroyed, so
// carrying on without it destroys the tree and the record of it together, and
// the caller learns that from a warning they have already scrolled past.
func TestARunStopsWhenItsLogCannotBeOpened(t *testing.T) {
	// Given: a tree, and a log path nothing can be written to.
	root := t.TempDir()
	victim := filepath.Join(root, "victim.txt")
	if err := os.WriteFile(victim, []byte("payload"), 0o600); err != nil {
		t.Fatalf("writing the victim: %v", err)
	}
	unwritable := filepath.Join(t.TempDir(), "no-such-directory", "manifest.log")

	// When: the run is asked to start.
	out, code := run(t, "--no-ui", "--yes", "--log", unwritable, root)

	// Then: it refuses.
	if code == 0 {
		t.Fatalf("a run with an unwritable log exited 0\n%s", out)
	}
	// And: the tree is untouched.
	body, err := os.ReadFile(victim)
	if err != nil || string(body) != "payload" {
		t.Fatalf("the run destroyed the tree anyway: contents are %q, err=%v", body, err)
	}
}

func moduleRoot() string {
	_, file, _, ok := runtime.Caller(0)
	if !ok {
		panic("unable to determine caller path")
	}
	return filepath.Clean(filepath.Join(filepath.Dir(file), ".."))
}
