package app

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"sync"
	"sync/atomic"
	"testing"

	"blank/internal/testsupport"
)

type result struct {
	err      error
	progress []float64
	activity []Event
}

// stagesFor returns, in order, the stages the run reported for one target.
func stagesFor(events []Event, path string) []Stage {
	var out []Stage
	for _, e := range events {
		if e.Path == path {
			out = append(out, e.Stage)
		}
	}
	return out
}

func runBlank(t *testing.T, args ...string) result {
	t.Helper()
	return runBlankWithJobs(t, DefaultJobs, args...)
}

// runBlankWithJobs drives the engine with a collector on each channel, so a test
// reads the outcome without repeating the fan-in.
func runBlankWithJobs(t *testing.T, jobs int, args ...string) result {
	t.Helper()
	progressCh := make(chan float64)
	activityCh := make(chan Event)

	var res result
	var wg sync.WaitGroup
	wg.Add(2)
	go func() {
		defer wg.Done()
		for p := range progressCh {
			res.progress = append(res.progress, p)
		}
	}()
	go func() {
		defer wg.Done()
		for msg := range activityCh {
			res.activity = append(res.activity, msg)
		}
	}()

	res.err = Blank(context.Background(), args, jobs, progressCh, activityCh)
	wg.Wait()
	return res
}

// The pool never runs more targets at once than it was told to.
// Given more files than the limit allows, when the run finishes, then the number
// of targets in flight never passed the limit.
// Why it matters: every target in flight is an open overwrite of a real file, so
// an unbounded pool starts hundreds at once. That thrashes the disk, and it fills
// the panel in one instant with rows nobody can read as they settle.
func TestJobsBoundsTheTargetsInFlight(t *testing.T) {
	testsupport.SetupStubCommands(t)

	for _, jobs := range []int{1, 4} {
		t.Run(fmt.Sprintf("jobs=%d", jobs), func(t *testing.T) {
			// Given: many more files than the limit allows in flight.
			root := t.TempDir()
			for i := range 24 {
				p := filepath.Join(root, fmt.Sprintf("file-%02d.bin", i))
				if err := os.WriteFile(p, []byte("payload"), 0o600); err != nil {
					t.Fatalf("writing %s: %v", p, err)
				}
			}

			// When: the run finishes.
			res := runBlankWithJobs(t, jobs, root)
			if res.err != nil {
				t.Fatalf("expected a clean run, got %v", res.err)
			}

			// Then: counting the stream up on a start and down on a settle never
			// passes the limit, because a settle is what frees a worker.
			live, peak := 0, 0
			for _, e := range res.activity {
				if e.Stage == StageStart {
					live++
					peak = max(peak, live)
					continue
				}
				live--
			}
			if peak > jobs {
				t.Fatalf("%d targets ran at once, the limit was %d", peak, jobs)
			}
		})
	}
}

// A pool of no workers is refused rather than quietly corrected.
// Given a jobs count below one, when the run is asked to start, then it returns
// an error and touches nothing.
// Why it matters: a count of zero queues every file against no worker at all, so
// the run would hang holding a list of files it reports as in progress.
func TestJobsBelowOneIsRefused(t *testing.T) {
	// Given: a file that must survive being named as a target.
	root := t.TempDir()
	victim := filepath.Join(root, "victim.txt")
	if err := os.WriteFile(victim, []byte("payload"), 0o600); err != nil {
		t.Fatalf("writing the victim: %v", err)
	}

	// When: the run is asked for a pool of no workers.
	res := runBlankWithJobs(t, 0, root)

	// Then: it refuses, naming the count it was given.
	if res.err == nil {
		t.Fatal("a pool of no workers was accepted")
	}
	if !strings.Contains(res.err.Error(), "jobs") {
		t.Fatalf("the error does not name the setting: %v", res.err)
	}
	// And: the file is still there, because nothing ran.
	if _, err := os.Stat(victim); err != nil {
		t.Fatalf("expected %s to survive a refused run: %v", victim, err)
	}
}

func TestBlankEmitsProgressAndActivity(t *testing.T) {
	logFile := testsupport.SetupStubCommands(t)

	root := t.TempDir()
	file1 := filepath.Join(root, "alpha.txt")
	file2 := filepath.Join(root, "nested", "beta.txt")
	if err := os.MkdirAll(filepath.Dir(file2), 0o755); err != nil {
		t.Fatalf("creating nested dir: %v", err)
	}
	if err := os.WriteFile(file1, []byte("secret-1"), 0o600); err != nil {
		t.Fatalf("writing file1: %v", err)
	}
	if err := os.WriteFile(file2, []byte("secret-2"), 0o600); err != nil {
		t.Fatalf("writing file2: %v", err)
	}

	symlink := filepath.Join(root, "link-to-alpha")
	if err := os.Symlink(file1, symlink); err != nil {
		t.Fatalf("creating symlink: %v", err)
	}

	res := runBlank(t, root)

	if res.err != nil {
		t.Fatalf("expected a clean run, got %v", res.err)
	}
	if len(res.progress) == 0 || res.progress[len(res.progress)-1] != 1 {
		t.Fatalf("expected final progress of 1, got %v", res.progress)
	}
	if len(res.activity) == 0 {
		t.Fatalf("expected activity messages, got none")
	}

	if _, err := os.Stat(file1); !os.IsNotExist(err) {
		t.Fatalf("expected %s to be removed, err=%v", file1, err)
	}
	if _, err := os.Stat(file2); !os.IsNotExist(err) {
		t.Fatalf("expected %s to be removed, err=%v", file2, err)
	}
	if _, err := os.Lstat(symlink); !os.IsNotExist(err) {
		t.Fatalf("expected symlink to be removed, err=%v", err)
	}

	data, err := os.ReadFile(logFile)
	if err != nil {
		t.Fatalf("reading stub log: %v", err)
	}
	if !strings.Contains(string(data), "shred "+file1) {
		t.Fatalf("expected shred log for %s, got %s", file1, string(data))
	}
}

// A directory the walk cannot read is reported, not skipped in silence.
// Given a directory with no read permission, when the run finishes, then it
// returns an error naming that directory and the files inside are still there.
// Why it matters: os.ReadDir failing looks exactly like an empty directory, so
// the contents are never queued, never overwritten, and never counted, and the
// run reports complete success over data that is still on disk.
func TestBlankReportsAnUnreadableDirectory(t *testing.T) {
	if os.Geteuid() == 0 {
		t.Skip("root ignores directory permissions, so the read cannot fail")
	}
	testsupport.SetupStubCommands(t)

	// Given: a tree holding one readable file and one directory nobody can read.
	root := t.TempDir()
	locked := filepath.Join(root, "locked")
	if err := os.MkdirAll(locked, 0o755); err != nil {
		t.Fatalf("creating locked dir: %v", err)
	}
	hidden := filepath.Join(locked, "hidden.txt")
	if err := os.WriteFile(hidden, []byte("still-here"), 0o600); err != nil {
		t.Fatalf("writing hidden file: %v", err)
	}
	if err := os.Chmod(locked, 0o000); err != nil {
		t.Fatalf("locking dir: %v", err)
	}
	// TempDir cannot clean up a directory it cannot enter.
	t.Cleanup(func() { _ = os.Chmod(locked, 0o755) })

	// When: the run finishes.
	res := runBlank(t, root)

	// Then: it reports the directory it could not read.
	if res.err == nil {
		t.Fatalf("unreadable directory reported success")
	}
	if !strings.Contains(res.err.Error(), locked) {
		t.Fatalf("error does not name the directory: %v", res.err)
	}
	// And: the file inside is untouched, which is what the error is about.
	// The test has to unlock the directory to look, exactly as the engine could
	// not.
	if err := os.Chmod(locked, 0o755); err != nil {
		t.Fatalf("unlocking dir: %v", err)
	}
	if body, err := os.ReadFile(hidden); err != nil || string(body) != "still-here" {
		t.Fatalf("expected the hidden file intact, got %q err=%v", body, err)
	}
}

// A directory the walk cannot list is reported even when its removal succeeds.
// Given a directory whose listing fails, when the run finishes, then it returns
// an error naming the listing failure, and no file inside was ever overwritten.
// Why it matters: the removal pass runs rm -rf, which sweeps the contents away
// whether or not the walk ever saw them, so a swallowed listing error destroys
// files without overwriting them and reports a clean run.
func TestBlankReportsADirectoryItCannotList(t *testing.T) {
	logFile := testsupport.SetupStubCommands(t)

	// Given: an ordinary tree that rm can remove without trouble.
	root := t.TempDir()
	sub := filepath.Join(root, "sub")
	if err := os.MkdirAll(sub, 0o755); err != nil {
		t.Fatalf("creating sub dir: %v", err)
	}
	inside := filepath.Join(sub, "inside.txt")
	if err := os.WriteFile(inside, []byte("payload"), 0o600); err != nil {
		t.Fatalf("writing inside file: %v", err)
	}

	// And: listing that one directory fails, which nothing else notices.
	real := readDir
	readDir = func(name string) ([]os.DirEntry, error) {
		if name == sub {
			return nil, errors.New("simulated listing failure")
		}
		return real(name)
	}
	t.Cleanup(func() { readDir = real })

	// When: the run finishes.
	res := runBlank(t, root)

	// Then: it reports the directory it could not list.
	if res.err == nil {
		t.Fatalf("unlistable directory reported success")
	}
	if !strings.Contains(res.err.Error(), "simulated listing failure") {
		t.Fatalf("error does not name the listing failure: %v", res.err)
	}
	// And: the file inside was removed without ever being overwritten, which is
	// the data loss the error exists to announce.
	data, err := os.ReadFile(logFile)
	if err != nil {
		t.Fatalf("reading stub log: %v", err)
	}
	if strings.Contains(string(data), "shred "+inside) {
		t.Fatalf("expected %s never to reach shred, got %s", inside, string(data))
	}
}

// A file the overwrite fails on is reported and left on disk.
// Given a file whose overwrite exits non-zero, when the run finishes, then it
// returns an error naming that file.
// Why it matters: counting a failed overwrite as done let the directory pass
// unlink the file with its contents intact, and the run still exited zero.
func TestBlankReportsAFailedOverwrite(t *testing.T) {
	testsupport.SetupStubCommands(t)

	// Given: two files, one of which the overwrite refuses.
	root := t.TempDir()
	doomed := filepath.Join(root, "ordinary.txt")
	stubborn := filepath.Join(root, "refuses.txt")
	for _, p := range []string{doomed, stubborn} {
		if err := os.WriteFile(p, []byte("payload"), 0o600); err != nil {
			t.Fatalf("writing %s: %v", p, err)
		}
	}
	t.Setenv(testsupport.FailMarker, "refuses.txt")

	// When: the run finishes.
	res := runBlank(t, root)

	// Then: it names the file the overwrite could not destroy.
	if res.err == nil {
		t.Fatalf("failed overwrite reported success")
	}
	if !strings.Contains(res.err.Error(), stubborn) {
		t.Fatalf("error does not name the file: %v", res.err)
	}
	// And: the file that did overwrite is gone, so the run was not abandoned.
	if _, err := os.Stat(doomed); !os.IsNotExist(err) {
		t.Fatalf("expected %s to be removed, err=%v", doomed, err)
	}
}

// Every target is announced when the run reaches it and again when it settles.
// Given one file that overwrites and one that refuses, when the run finishes,
// then each reports a start and then its own outcome.
// Why it matters: a target announced only at the start reads as destroyed the
// moment the work is queued, so a reader is told a file is gone while the
// overwrite is still running, or after it has already failed.
func TestBlankReportsTheOutcomeOfEachTarget(t *testing.T) {
	testsupport.SetupStubCommands(t)

	// Given: two files, one of which the overwrite refuses.
	root := t.TempDir()
	ordinary := filepath.Join(root, "ordinary.txt")
	stubborn := filepath.Join(root, "refuses.txt")
	for _, p := range []string{ordinary, stubborn} {
		if err := os.WriteFile(p, []byte("payload"), 0o600); err != nil {
			t.Fatalf("writing %s: %v", p, err)
		}
	}
	t.Setenv(testsupport.FailMarker, "refuses.txt")

	// When: the run finishes.
	res := runBlank(t, root)

	// Then: the file that overwrote is reported as done, and only once it is.
	if got := stagesFor(res.activity, ordinary); !slices.Equal(got, []Stage{StageStart, StageDone}) {
		t.Fatalf("the file that overwrote reported %v", got)
	}
	// And: the file that survived is reported as failed, so the row cannot read
	// as destroyed.
	if got := stagesFor(res.activity, stubborn); !slices.Equal(got, []Stage{StageStart, StageFailed}) {
		t.Fatalf("the file that survived reported %v", got)
	}
	// And: a directory settles the same way, so both passes report alike.
	if got := stagesFor(res.activity, root); !slices.Equal(got, []Stage{StageStart, StageDone}) {
		t.Fatalf("the directory reported %v", got)
	}
}

// peakGoroutines destroys a flat tree of n files and returns the highest live
// goroutine count seen while it ran.
func peakGoroutines(t *testing.T, n int) int64 {
	t.Helper()
	root := t.TempDir()
	for i := range n {
		p := filepath.Join(root, fmt.Sprintf("file-%05d.txt", i))
		if err := os.WriteFile(p, []byte("payload"), 0o600); err != nil {
			t.Fatalf("writing %s: %v", p, err)
		}
	}

	var peak atomic.Int64
	stop := make(chan struct{})
	watching := make(chan struct{})
	go func() {
		defer close(watching)
		for {
			if live := int64(runtime.NumGoroutine()); live > peak.Load() {
				peak.Store(live)
			}
			select {
			case <-stop:
				return
			default:
			}
		}
	}()

	res := runBlank(t, root)
	close(stop)
	<-watching

	if res.err != nil {
		t.Fatalf("expected a clean run over %d files, got %v", n, res.err)
	}
	return peak.Load()
}

// The engine bounds the goroutines it creates, not only the work in flight.
// Given one small tree and one four times larger, when each is destroyed, then
// the peak goroutine count is the same for both.
// Why it matters: a goroutine per file allocated a stack for every file in the
// tree before any overwrite began, so the cost grew with the tree while only the
// concurrency limit could ever be doing work.
func TestBlankBoundsItsGoroutines(t *testing.T) {
	testsupport.SetupStubCommands(t)

	// Given / When: two runs whose file counts differ by a factor of four.
	small := peakGoroutines(t, DefaultJobs*2)
	large := peakGoroutines(t, DefaultJobs*8)

	// Then: the peak tracks the concurrency limit, not the file count. Comparing
	// the two runs avoids pinning a number, which would only describe how many
	// goroutines os/exec happens to use per command today.
	if large > small+DefaultJobs {
		t.Fatalf("peak goroutines grew with the tree: %d for %d files, %d for %d files",
			small, DefaultJobs*2, large, DefaultJobs*8)
	}
}

// A filename cannot carry a terminal escape into what the run prints.
// Given a file whose name holds control characters, when the run reports it,
// then neither the activity stream nor the error carries a control character.
// Why it matters: both are written straight to a terminal, so a crafted name
// could set the window title, move the cursor, or hide the rest of the line,
// and the tree being destroyed is often not the operator's own.
func TestBlankEscapesControlCharactersInNames(t *testing.T) {
	testsupport.SetupStubCommands(t)

	// Given: a file whose name holds an escape sequence and a bell. Every byte
	// except a slash and a null is legal in a name, so this is a real file.
	root := t.TempDir()
	nasty := filepath.Join(root, "evil\x1b]0;hijacked\x07\x08\x0a.txt")
	if err := os.WriteFile(nasty, []byte("payload"), 0o600); err != nil {
		t.Fatalf("writing the crafted file: %v", err)
	}
	t.Setenv(testsupport.FailMarker, "evil")

	// When: the run reports it, both as activity and as a survivor.
	res := runBlank(t, root)

	// Then: nothing the run emits carries a control character.
	if res.err == nil {
		t.Fatalf("expected the refused overwrite to be reported")
	}
	emitted := []string{res.err.Error()}
	for _, e := range res.activity {
		emitted = append(emitted, e.Verb+" "+e.Path)
	}
	for _, line := range emitted {
		for _, r := range line {
			if r == '\n' {
				continue
			}
			if r < 0x20 || r == 0x7f || (r >= 0x80 && r <= 0x9f) {
				t.Fatalf("control character %#U reached the terminal in %q", r, line)
			}
		}
	}
	// And: the name is still recognisable, so the operator can act on it.
	joined := strings.Join(emitted, "\n")
	if !strings.Contains(joined, "evil") || !strings.Contains(joined, ".txt") {
		t.Fatalf("the escaped name lost its readable parts: %q", joined)
	}
}

// An argument that does not exist is reported rather than ignored.
// Given a path with nothing at it, when the run finishes, then it returns an
// error naming the path and closes both channels.
// Why it matters: a mistyped argument used to exit zero having destroyed
// nothing, which reads exactly like a successful run.
func TestBlankReportsAMissingArgument(t *testing.T) {
	// Given / When: a path that was never created.
	missing := filepath.Join(t.TempDir(), "does-not-exist")
	res := runBlank(t, missing)

	// Then: the run reports it. Reaching this line at all proves both channels
	// closed, because runBlank waits on them.
	if res.err == nil {
		t.Fatalf("missing argument reported success")
	}
	if !strings.Contains(res.err.Error(), missing) {
		t.Fatalf("error does not name the path: %v", res.err)
	}
	// And: nothing was reported as progress, because there was no work.
	if len(res.progress) != 0 {
		t.Fatalf("expected no progress, got %v", res.progress)
	}
}
