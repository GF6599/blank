// Package app contains the secure deletion engine backing the CLI and TUI.
package app

import (
	"context"
	"errors"
	"fmt"
	"io/fs"
	"log/slog"
	"os"
	"slices"
	"strings"
	"sync"
	"sync/atomic"
)

// DefaultJobs is how many targets are overwritten at the same time. The work is
// disk bound, so the best count is a property of the device rather than of the
// machine, which is why --jobs exists and why this is not read from the processor
// count. Measured over 120 files of 1 MiB on an NVMe disk: 16 workers took 136ms,
// 8 took 143ms, 100 took 177ms, and 4 took 216ms.
const DefaultJobs = 16

// DefaultPasses is how many random covers each file gets before the zero cover,
// matching what `shred -uzfn 10` performed when the overwrite was a subprocess.
const DefaultPasses = 10

var (
	// A listing failure is the one fault the filesystem cannot be made to
	// produce on demand, so the walk reads the directory through this.
	readDir = os.ReadDir
)

// Stage is how far the run has got with one target.
type Stage int

const (
	StageStart Stage = iota
	StageDone
	StageFailed
)

func (s Stage) String() string {
	switch s {
	case StageStart:
		return "start"
	case StageDone:
		return "done"
	case StageFailed:
		return "failed"
	default:
		return fmt.Sprintf("Stage(%d)", int(s))
	}
}

// Event reports one target as the run reaches it and again as it settles. Path
// identifies the target across both, and is already safe to write to a terminal.
type Event struct {
	Verb  string
	Path  string
	Stage Stage
}

// The wording of an activity row, kept here so the two events for one target
// cannot describe it differently.
const (
	verbFile    = "Overwriting"
	verbDir     = "Deleting directory"
	verbRemoval = "Removing"
)

// Failure names one target the run did not destroy.
type Failure struct {
	Path string
	Err  error
}

// IncompleteRun reports the targets that survived. The caller believes they are
// gone, so the error names them rather than only counting them.
type IncompleteRun struct {
	Failures []Failure
}

// Past this many the message stops being readable, and the log holds the rest.
const failuresNamed = 10

func (r *IncompleteRun) Error() string {
	noun := "targets"
	if len(r.Failures) == 1 {
		noun = "target"
	}
	var b strings.Builder
	fmt.Fprintf(&b, "%d %s survived the run", len(r.Failures), noun)
	for i, f := range r.Failures {
		if i == failuresNamed {
			fmt.Fprintf(&b, "\n  and %d more", len(r.Failures)-failuresNamed)
			break
		}
		fmt.Fprintf(&b, "\n  %s", displayText(fmt.Sprintf("%s: %v", f.Path, f.Err)))
	}
	return b.String()
}

// unsafeForTerminal reports the C0 controls, DEL, and the C1 range.
func unsafeForTerminal(r rune) bool {
	return r < 0x20 || r == 0x7f || (r >= 0x80 && r <= 0x9f)
}

// displayText makes text safe to write to a terminal. A filename is chosen by
// whoever wrote the file, not by whoever destroys it, and a control character
// inside one can set the window title, move the cursor, or hide the rest of the
// line. Every escaped rune is below 0x100, so two hex digits always fit.
func displayText(s string) string {
	if strings.IndexFunc(s, unsafeForTerminal) < 0 {
		return s
	}
	var b strings.Builder
	b.Grow(len(s))
	for _, r := range s {
		if unsafeForTerminal(r) {
			fmt.Fprintf(&b, `\x%02x`, r)
			continue
		}
		b.WriteRune(r)
	}
	return b.String()
}

// failures collects survivors from every goroutine that overwrites a file.
type failures struct {
	mu   sync.Mutex
	list []Failure
}

func (f *failures) add(path string, err error) {
	slog.Error("Target survived", "PATH", displayText(path), "ERR", err)
	f.mu.Lock()
	defer f.mu.Unlock()
	f.list = append(f.list, Failure{Path: path, Err: err})
}

func (f *failures) err() error {
	f.mu.Lock()
	defer f.mu.Unlock()
	if len(f.list) == 0 {
		return nil
	}
	return &IncompleteRun{Failures: slices.Clone(f.list)}
}

// Options are the knobs a run is given. A struct rather than positional
// arguments, because two bare ints side by side in the signature can be swapped
// by a caller with nothing to catch it.
type Options struct {
	// Jobs is how many targets are worked on at the same time.
	Jobs int
	// Passes is how many random covers a file gets before the zero cover.
	Passes int
}

// Blank destroys everything the plan holds, streaming progress and activity as
// it goes. Either channel may be nil, for a caller that wants only the outcome.
// It returns an error naming every target that survived, including the ones the
// enumeration behind the plan already could not read.
func Blank(ctx context.Context, plan Plan, opts Options, progress chan<- float64, activity chan<- Event) error {
	// Refused rather than clamped, and refused here rather than per target, so a
	// bad value stops the run before it destroys anything.
	if err := opts.Validate(); err != nil {
		closeStreams(progress, activity)
		return err
	}
	fails := &failures{list: slices.Clone(plan.Failures)}
	if plan.Total() == 0 {
		slog.Info("Nothing to destroy")
		closeStreams(progress, activity)
		return fails.err()
	}
	slog.Info("Targets to destroy", "TOTAL", plan.Total())
	concurrentShred(ctx, plan, opts, fails, progress, activity)
	if err := fails.err(); err != nil {
		return err
	}
	slog.Info("blank finished. Every target was destroyed.")
	return nil
}

// closeStreams ends both streams. A reader ranges over them, so a run that
// returns without closing them leaves that reader waiting for ever.
func closeStreams(progress chan<- float64, activity chan<- Event) {
	if progress != nil {
		close(progress)
	}
	if activity != nil {
		close(activity)
	}
}

// os.Remove never follows a symlink to the file it names, and it never opens
// the entry, so a FIFO with no writer cannot stall the run.
func removeEntry(path string) error {
	if err := os.Remove(path); err != nil && !errors.Is(err, fs.ErrNotExist) {
		return fmt.Errorf("remove: %w", err)
	}
	return nil
}

// removeDir removes one directory and never its contents. os.Remove refuses a
// directory that still holds anything, which is what turns a target the run
// failed to destroy into a reported failure. `rm -rf` removed the directory
// either way, so a file whose contents were never overwritten was unlinked all
// the same: the caller lost the file and the plaintext stayed on the disk.
func removeDir(path string) error {
	if err := os.Remove(path); err != nil && !errors.Is(err, fs.ErrNotExist) {
		return fmt.Errorf("remove directory: %w", err)
	}
	return nil
}

// A stop is not a success: everything behind it is still on disk, and the
// caller reads the exit code rather than the interface.
func recordSkipped(fails *failures, paths []string, err error) {
	for _, path := range paths {
		fails.add(path, err)
	}
}

// concurrentShred fans work out to a fixed pool of workers, and emits live
// progress plus optional activity updates.
func concurrentShred(ctx context.Context, plan Plan, opts Options, fails *failures, progress chan<- float64, activity chan<- Event) {
	if ctx == nil {
		ctx = context.Background()
	}
	defer closeStreams(progress, activity)
	var wg sync.WaitGroup

	total := plan.Total()
	var done atomic.Int64

	sendEvent := func(verb, path string, stage Stage) {
		if activity == nil {
			return
		}
		select {
		case <-ctx.Done():
			return
		case activity <- Event{Verb: verb, Path: displayText(path), Stage: stage}:
		}
	}

	sendProgress := func(pct float64) {
		// A send on a nil channel waits for ever, which is a hang rather than
		// the panic a missing stream would otherwise announce.
		if progress == nil {
			return
		}
		select {
		case <-ctx.Done():
			return
		case progress <- pct:
		}
	}

	// runTarget announces the target, does the work, then announces how it
	// ended. One branch decides both the stage and the failure record, so the
	// panel cannot settle a row that fails or leave a destroyed one running.
	runTarget := func(verb, path string, work func() error) {
		sendEvent(verb, path, StageStart)
		stage := StageDone
		if err := work(); err != nil {
			fails.add(path, err)
			stage = StageFailed
		}
		// One line per target, whatever became of it. This is the manifest
		// --log exists to write, and it is the only record of what the run
		// reached once the files themselves are gone.
		slog.Info(verb, "PATH", displayText(path), "STAGE", stage)
		sendEvent(verb, path, stage)
	}

	update := func() {
		// Increment completed items (files + directories)
		d := done.Add(1)
		pct := float64(d) / float64(total)
		if pct > 1 {
			pct = 1
		}
		sendProgress(pct)
		slog.Info("Overall progress", "PERCENT", pct)
	}

	// Workers pull from the queue rather than one goroutine starting per file.
	// A semaphore bounds the work in flight but not the goroutines, so a large
	// tree used to allocate a stack for every file before any overwrite began.
	queue := make(chan string)
	for range min(opts.Jobs, len(plan.Files)) {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for f := range queue {
				if err := ctx.Err(); err != nil {
					fails.add(f, err)
					continue
				}
				runTarget(verbFile, f, func() error {
					return overwriteFile(ctx, f, opts.Passes)
				})
				update()
			}
		}()
	}

feed:
	for i, f := range plan.Files {
		select {
		case <-ctx.Done():
			recordSkipped(fails, plan.Files[i:], ctx.Err())
			break feed
		case queue <- f:
		}
	}
	close(queue)
	wg.Wait()

	// Removals run before the directories that hold them, because a directory
	// is only removable once it is empty.
	for i, r := range plan.Removals {
		if err := ctx.Err(); err != nil {
			recordSkipped(fails, plan.Removals[i:], err)
			break
		}
		runTarget(verbRemoval, r, func() error { return removeEntry(r) })
		update()
	}

	// Deepest first. walk appends a directory before the entries inside it, so
	// reading its output backwards always reaches a child before its parent,
	// which is the order os.Remove needs.
	deepestFirst := slices.Clone(plan.Directories)
	slices.Reverse(deepestFirst)
	for i, dir := range deepestFirst {
		if err := ctx.Err(); err != nil {
			recordSkipped(fails, deepestFirst[i:], err)
			break
		}
		runTarget(verbDir, dir, func() error { return removeDir(dir) })
		update()
	}
}

// Validate reports a setting a run refuses. The command calls it before it
// enumerates, so a bad flag stops the run before it reads or destroys anything.
func (o Options) Validate() error {
	if o.Jobs < 1 {
		// A pool of no workers queues every file against nothing, and the run
		// hangs reporting them all as in progress.
		return fmt.Errorf("jobs must be at least 1, got %d", o.Jobs)
	}
	if o.Passes < 0 {
		return fmt.Errorf("passes cannot be negative, got %d", o.Passes)
	}
	return nil
}
