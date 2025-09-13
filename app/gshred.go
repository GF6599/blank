// Package app
// Contains the functionality of shreding
package app

import (
	"fmt"
	"io/fs"
	"log/slog"
	"os"
	"os/exec"
	"path/filepath"
	"sync"
)

type (
	file      string
	directory string
)

type work struct {
	files       []file
	directories []directory
}

func (w *work) len() int {
	return len(w.files) + len(w.directories)
}

// Gshred expands the CLI args and find all the individual files.
// The files will then be concurrently shredded.
func Gshred(args []string) {
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
			if err := walk(w, absolutePath); err != nil {
				slog.Error("Failed to walk the directory", "PATH", absolutePath, "ERR", err)
			}
		} else {
			w.files = append(w.files, file(absolutePath))
		}
	}
	if w.len() == 0 {
		slog.Info("No files or directories to shred")
		return
	}
	slog.Info("Number of files / directories to shred", "TOTAL", w.len())
	concurrentShred(w)
	slog.Info("gShred finished! All files and dir deleted.")
}

// Delete a directory by performing rm -rf command
func shredDir(d directory) error {
	slog.Info("Deleting directory", "DIR", string(d))
	cmd := exec.Command("rm", "-rf", string(d))
	if out, err := cmd.CombinedOutput(); err != nil {
		return fmt.Errorf("rm -rf failed: %w (output: %s)", err, string(out))
	}
	return nil
}

// Shred a file by performing the shred command
func shredFile(f file) error {
	cmd := exec.Command("shred", "-uzfn", "10", string(f))
	if out, err := cmd.CombinedOutput(); err != nil {
		return fmt.Errorf("shred failed: %w (output: %s)", err, string(out))
	}
	return nil
}

// Given a list of files to shred, perform the shred Command
// on files concurrently, and then perform rm -rf on any
// directories afterwards
func concurrentShred(w *work) {
	// Channel for reporting which file is being operated on.
	stats := make(chan string, 128)
	// Semaphore to limit the amount of concurrent shredding happening.
	sema := make(chan struct{}, 100)
	// Create waitgroup for all goroutines to finish
	var wg sync.WaitGroup
	// Progress goroutine
	progressDone := make(chan struct{})
	go func(total int) {
		defer close(progressDone)
		var done int
		for range stats {
			done++
			// Avoid division by zero (already guarded before) but keep safe
			if total > 0 {
				pct := (float64(done) / float64(total)) * 100
				slog.Info("Overall progress", "PERCENT", pct)
			}
		}
	}(w.len())
	// Loop over all the files and shred them concurrently.
	for _, f := range w.files {
		wg.Add(1)
		go func(f file) {
			sema <- struct{}{}
			defer func() {
				<-sema
				stats <- fmt.Sprintf("File: %s", f)
				wg.Done()
			}()
			if err := shredFile(f); err != nil {
				if stat, serr := os.Stat(string(f)); serr == nil {
					slog.Error("Failed to shred file", "FILE", f, "MODE", stat.Mode(), "ERR", err)
				} else {
					slog.Error("Failed to shred file", "FILE", f, "STAT_ERR", serr, "ERR", err)
				}
			}
		}(f)
	}
	// Wait for all file shredding goroutines to finish
	wg.Wait()
	// Delete the folders after all files are shredded
	for _, dir := range w.directories {
		if err := shredDir(dir); err != nil {
			slog.Error("Failed to delete dir", "DIR", dir, "ERR", err)
		}
		stats <- fmt.Sprintf("Directory: %s", dir)
	}
	// All sends complete; safe to close stats
	close(stats)
	<-progressDone // ensure progress logging completes before return
}

// Get the contents of a directory.
func dirents(dir string) []fs.DirEntry {
	files, err := os.ReadDir(dir)
	if err != nil {
		slog.Warn("Failed to read dir contents", "DIR", dir, "ERR", err)
		return nil
	}
	return files
}

// Checks whether the given path is a dir.
func isDir(path string) (bool, error) {
	f, err := os.Lstat(path)
	if err != nil {
		slog.Warn("Failed to check isdir", "PATH", path, "ERR", err)
		return false, err
	}
	return f.IsDir(), nil
}

// Check if a given path is a symlink
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

// Walk a directory and collect all files & directories (recursive)
// Symbolic links are removed via rm -rf and NOT shredded.
func walk(w *work, dir string) error {
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
			cmd := exec.Command("rm", "-rf", fp)
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
			if err := walk(w, fp); err != nil {
				return err
			}
			continue
		}
		w.files = append(w.files, file(fp))
	}
	return nil
}
