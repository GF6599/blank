// Package app contains the secure deletion engine backing the CLI and TUI.
package app

import (
	"context"
	"fmt"
	"io/fs"
	"log/slog"
	"os"
	"os/exec"
	"path/filepath"
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
)

// work groups files and directories queued for secure deletion.
type work struct {
	files       []file
	directories []directory
}

// len returns the total count of files and directories in the work queue.
func (w *work) len() int {
	return len(w.files) + len(w.directories)
}

// Gshred expands CLI arguments, walks directories, and securely deletes each
// item while streaming progress percentages and human-readable activity updates.
func Gshred(ctx context.Context, args []string, progress chan<- float64, activity chan<- string) {
	// Local workload to avoid global mutable state accumulation / data races
	w := &work{}
	for _, arg := range args {
		// Convert to absolute path
		absolutePath, err := filepath.Abs(arg)
		if err != nil {
			slog.Error("Failed to find absolute path of file", "PATH", arg, "ERR", err)
			continue
		}
		isdir, err := isDir(absolutePath)
		if err != nil {
			slog.Error("Failed to check weather path is a dir", "PATH", absolutePath, "ERR", err)
			continue
		}
		if isdir {
			if err := walk(ctx, w, absolutePath); err != nil {
				slog.Error("Failed to walk the directory", "PATH", absolutePath, "ERR", err)
			}
		} else {
			w.files = append(w.files, file(absolutePath))
		}
	}
	if w.len() == 0 {
		slog.Info("No files or directories to shred")
		close(progress)
		if activity != nil {
			close(activity)
		}
		return
	}
	slog.Info("Number of files / directories to shred", "TOTAL", w.len())
	concurrentShred(ctx, w, progress, activity)
	slog.Info("gShred finished! All files and dir deleted.")
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
func concurrentShred(ctx context.Context, w *work, progress chan<- float64, activity chan<- string) {
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
	if total == 0 {
		return
	}
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
				update() // count this file (whether success or failure)
				wg.Done()
			}()
			if err := ctx.Err(); err != nil {
				return
			}
			sendActivity("Shredding %s", string(f))
			if err := shredFile(ctx, f); err != nil {
				if stat, serr := os.Stat(string(f)); serr == nil {
					slog.Error("Failed to shred file", "FILE", f, "MODE", stat.Mode(), "ERR", err)
				} else {
					slog.Error("Failed to shred file", "FILE", f, "STAT_ERR", serr, "ERR", err)
				}
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
			slog.Error("Failed to delete dir", "DIR", dir, "ERR", err)
		}
		update()
	}
}

// dirents reads directory entries and logs any failure.
func dirents(dir string) []fs.DirEntry {
	files, err := os.ReadDir(dir)
	if err != nil {
		slog.Warn("Failed to read dir contents", "DIR", dir, "ERR", err)
		return nil
	}
	return files
}

// isDir reports whether the given path refers to a directory.
func isDir(path string) (bool, error) {
	f, err := os.Lstat(path)
	if err != nil {
		slog.Warn("Failed to check isdir", "PATH", path, "ERR", err)
		return false, err
	}
	return f.IsDir(), nil
}

// isSymLink reports whether the path is a symbolic link.
func isSymLink(path string) (bool, error) {
	f, err := os.Lstat(path)
	if err != nil {
		slog.Warn("Failed to check isSymLink", "PATH", path, "ERR", err)
		return false, err
	}
	if f.Mode()&os.ModeSymlink != 0 {
		slog.Warn("Symlink encountered", "PATH", path)
		return true, nil
	}
	return false, nil
}

// walk recursively collects files/directories, following actual directories but
// deleting symbolic links to avoid shredding the linked target.
func walk(ctx context.Context, w *work, dir string) error {
	w.directories = append(w.directories, directory(dir))
	de := dirents(dir)
	for _, f := range de {
		fp := filepath.Join(dir, f.Name())

		// Skip / remove symlinks
		if isSymbolicLink, err := isSymLink(fp); err != nil {
			slog.Error("Problem reading info on path", "PATH", fp, "ERR", err)
			continue
		} else if isSymbolicLink {
			slog.Info("Symbolic link detected, removing with 'rm -rf'", "PATH", fp)
			cmd := exec.CommandContext(ctx, rmBinary, "-rf", fp)
			if out, err := cmd.CombinedOutput(); err != nil {
				slog.Error("Failed to remove symbolic link", "PATH", fp, "ERR", err, "OUT", string(out))
			}
			continue
		}

		isDirectory, err := isDir(fp)
		if err != nil {
			slog.Warn("Ignoring error and continuing walk", "PATH", fp, "ERR", err)
			continue
		}
		if isDirectory {
			// Do not append here again; walk() will append when entering
			if err := walk(ctx, w, fp); err != nil {
				return err
			}
			continue
		}
		w.files = append(w.files, file(fp))
	}
	return nil
}
