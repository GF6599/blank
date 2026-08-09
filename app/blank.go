// Package app contains the secure deletion engine backing the CLI and TUI.
package app

import (
	"context"
	"errors"
	"fmt"
	"io/fs"
	"log/slog"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"sync/atomic"
)

type (
	file      string
	directory string
)

var (
	shredBinary = "shred"
	rmBinary    = "rm"
	// A listing failure is the one fault the filesystem cannot be made to
	// produce on demand, so the walk reads the directory through this.
	readDir = os.ReadDir
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
		fmt.Fprintf(&b, "\n  %s: %v", f.Path, f.Err)
	}
	return b.String()
}

// failures collects survivors from every goroutine that overwrites a file.
type failures struct {
	mu   sync.Mutex
	list []Failure
}

func (f *failures) add(path string, err error) {
	slog.Error("Target survived", "PATH", path, "ERR", err)
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

// work groups files and directories queued for secure deletion.
type work struct {
	files       []file
	directories []directory
}

// len returns the total count of files and directories in the work queue.
func (w *work) len() int {
	return len(w.files) + len(w.directories)
}

type kind int

const (
	kindFile kind = iota
	kindDir
	kindSymlink
)

// classify reads the path once. The walk needs both answers for every entry,
// and a single Lstat carries them.
func classify(path string) (kind, error) {
	fi, err := os.Lstat(path)
	if err != nil {
		return kindFile, err
	}
	switch {
	case fi.Mode()&os.ModeSymlink != 0:
		return kindSymlink, nil
	case fi.IsDir():
		return kindDir, nil
	default:
		return kindFile, nil
	}
}

// Blank expands CLI arguments, walks directories, and securely deletes each
// item while streaming progress percentages and human-readable activity
// updates. It returns an error naming every target that survived.
func Blank(ctx context.Context, args []string, progress chan<- float64, activity chan<- string) error {
	// Local workload to avoid global mutable state accumulation / data races
	w := &work{}
	fails := &failures{}
	for _, arg := range args {
		// Convert to absolute path
		absolutePath, err := filepath.Abs(arg)
		if err != nil {
			fails.add(arg, fmt.Errorf("resolve path: %w", err))
			continue
		}
		k, err := classify(absolutePath)
		if err != nil {
			fails.add(absolutePath, fmt.Errorf("read path: %w", err))
			continue
		}
		if k == kindDir {
			walk(w, fails, absolutePath)
			continue
		}
		w.files = append(w.files, file(absolutePath))
	}
	if w.len() == 0 {
		slog.Info("No files or directories to shred")
		close(progress)
		if activity != nil {
			close(activity)
		}
		return fails.err()
	}
	slog.Info("Number of files / directories to shred", "TOTAL", w.len())
	concurrentShred(ctx, w, fails, progress, activity)
	if err := fails.err(); err != nil {
		return err
	}
	slog.Info("blank finished. All files and directories deleted.")
	return nil
}

// shredDir removes a directory tree by invoking `rm -rf` via the provided context.
func shredDir(ctx context.Context, d directory) error {
	slog.Info("Deleting directory", "DIR", string(d))
	cmd := exec.CommandContext(ctx, rmBinary, "-rf", string(d))
	if out, err := cmd.CombinedOutput(); err != nil {
		return fmt.Errorf("rm -rf failed: %w (output: %s)", err, string(out))
	}
	return nil
}

// shredFile overwrites and removes a file by calling the system `shred` command.
func shredFile(ctx context.Context, f file) error {
	cmd := exec.CommandContext(ctx, shredBinary, "-uzfn", "10", string(f))
	if out, err := cmd.CombinedOutput(); err != nil {
		return fmt.Errorf("shred failed: %w (output: %s)", err, string(out))
	}
	return nil
}

// concurrentShred fan-outs shredding work, limits in-flight jobs via a
// semaphore, and emits live progress plus optional activity updates.
func concurrentShred(ctx context.Context, w *work, fails *failures, progress chan<- float64, activity chan<- string) {
	if ctx == nil {
		ctx = context.Background()
	}
	defer close(progress)
	if activity != nil {
		defer close(activity)
	}
	// Semaphore to limit the amount of concurrent shredding happening.
	sema := make(chan struct{}, 100)
	var wg sync.WaitGroup

	total := w.len()
	var done int64

	sendActivity := func(format string, args ...any) {
		if activity == nil {
			return
		}
		msg := fmt.Sprintf(format, args...)
		select {
		case <-ctx.Done():
			return
		case activity <- msg:
		}
	}

	sendProgress := func(pct float64) {
		select {
		case <-ctx.Done():
			return
		case progress <- pct:
		}
	}

	update := func() {
		// Increment completed items (files + directories)
		d := atomic.AddInt64(&done, 1)
		pct := float64(d) / float64(total)
		if pct > 1 {
			pct = 1
		}
		sendProgress(pct)
		slog.Info("Overall progress", "PERCENT", pct)
	}

	// Launch file shredders
	for _, f := range w.files {
		wg.Add(1)
		go func(f file) {
			select {
			case <-ctx.Done():
				wg.Done()
				return
			case sema <- struct{}{}:
			}
			defer func() {
				<-sema
				update()
				wg.Done()
			}()
			if err := ctx.Err(); err != nil {
				return
			}
			sendActivity("Overwriting %s", string(f))
			if err := shredFile(ctx, f); err != nil {
				if stat, serr := os.Stat(string(f)); serr == nil {
					err = fmt.Errorf("%w (mode %s)", err, stat.Mode())
				}
				fails.add(string(f), err)
			}
		}(f)
	}

	// Wait until all file goroutines finish
	wg.Wait()

	// Now delete directories (sequentially) and update progress after each
	for _, dir := range w.directories {
		if err := ctx.Err(); err != nil {
			break
		}
		sendActivity("Deleting directory %s", string(dir))
		if err := shredDir(ctx, dir); err != nil {
			fails.add(string(dir), err)
		}
		update()
	}
}

// walk collects the tree under dir. A directory it cannot read is recorded
// rather than skipped, because os.ReadDir failing looks exactly like an empty
// directory and the files inside would otherwise never be queued.
func walk(w *work, fails *failures, dir string) {
	w.directories = append(w.directories, directory(dir))
	entries, err := readDir(dir)
	if err != nil {
		fails.add(dir, fmt.Errorf("read directory: %w", err))
		return
	}
	for _, e := range entries {
		fp := filepath.Join(dir, e.Name())
		k, err := classify(fp)
		if err != nil {
			fails.add(fp, fmt.Errorf("read path: %w", err))
			continue
		}
		switch k {
		case kindSymlink:
			// os.Remove unlinks the link and never follows it to the target.
			// Unlike rm -rf it also reports a removal that failed.
			if err := os.Remove(fp); err != nil && !errors.Is(err, fs.ErrNotExist) {
				fails.add(fp, fmt.Errorf("remove symlink: %w", err))
			}
		case kindDir:
			walk(w, fails, fp)
		default:
			w.files = append(w.files, file(fp))
		}
	}
}
