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

type file string
type directory string

type work struct {
	files       []file
	directories []directory
}

func (w *work) len() int {
	return len(w.files) + len(w.directories)
}

var (
	workload work
)

// Expand the CLI args and find all the individual files.
// The files will then be concurrently shredded.
func Gshred(args []string) {
	// var toShred []string
	for _, arg := range args {
		// Convert to absolute path
		absolutePath, err := filepath.Abs(arg)
		if err != nil {
			slog.Error("Failed to find absolute path of file: ", arg, err)
		}
		isdir, err := isdir(absolutePath)
		if err != nil {
			slog.Error("Provided path: ", arg, " is ill formed!")
		}
		if isdir {
			if err := walk(arg); err != nil {
				slog.Error("Failed to walk the directory: ", arg)
			}
		} else {
			workload.files = append(workload.files, file(arg))
		}
	}
	slog.Info("Number of files to shred: %d", workload.len())
	concurrentShred(workload)
	slog.Info("gShred finished! All files and dir deleted.")
}

// Delete a directory by performing rm -rf command
func shredDir(d directory) error {
	// TODO:
	// Make this more secure?
	// Maybe mimic shred behaviour and rename the dir to len(dir_name) 0's
	// and then reduce the length by 1 each rename until len(dir_name) is 1
	// then remove the dir
	slog.Info("Deleting dir: ", d)
	cmd := exec.Command("rm", "-rf", string(d))
	if err := cmd.Run(); err != nil {
		return err
	}
	return nil
}

// Shred a file by performing the shred command
func shredFile(f file) error {
	cmd := exec.Command("shred", "-uzfn", "10", string(f))
	if err := cmd.Run(); err != nil {
		return err
	}
	return nil
}

// Given a list of files to shred, perform the shred Command
// on files concurrently, and then perform rm -rf on any
// directories afterwards
func concurrentShred(workload work) {
	// Channel for reporting which file is being operated on.
	stats := make(chan string)
	// Semaphore to limit the amount of concurrent shreding happening.
	sema := make(chan struct{}, 100)
	// Clean up the channels
	defer close(stats)
	defer close(sema)
	// Create waitgroup for all goroutines to finish
	var wg sync.WaitGroup
	// Go routine to print out the overall progress.
	go func() {
		var done int
		for range stats {
			done++
			slog.Info("%.2f%%\n", (float64(done)/float64(workload.len()))*100)
		}
	}()
	// Loop over all the files and shred them concurrently.
	for _, f := range workload.files {
		f := f
		// Start a goroutine to shred the file
		wg.Add(1)
		go func(f file) {
			sema <- struct{}{}
			defer func() {
				<-sema
				stats <- fmt.Sprintf("File: %s", f)
				wg.Done()
			}()
			if err := shredFile(f); err != nil {
				stat, _ := os.Stat(string(f))
				slog.Error("Failed to shred file: ", f, stat.Mode())
			}
		}(f)
	}
	// Wait for all goroutine to finish
	wg.Wait()
	// Delete the folders after all files are shreded
	for _, dir := range workload.directories {
		if err := shredDir(dir); err != nil {
			slog.Error("Failed to delete dir: ", dir)
		}
		stats <- fmt.Sprintf("Directory: %s", dir)
	}
}

// Get the contents of a directory.
func dirents(dir string) []fs.DirEntry {
	files, err := os.ReadDir(string(dir))
	if err != nil {
		slog.Warn("Failed to read dir contents: ", dir)
		return nil
	}
	return files
}

// Checks whether the given path is a dir.
func isdir(path string) (bool, error) {
	f, err := os.Lstat(path)
	if err != nil {
		slog.Warn("Failed to check isdir!", path, err)
		return false, err
	}
	if f.IsDir() {
		return true, nil
	}
	return false, nil
}

// Check if a given path is a symlink
func isSymLink(path string) (bool, error) {
	f, err := os.Lstat(path)
	if err != nil {
		slog.Warn("Failed to check isSymLink!", path, err)
		return false, err
	}
	if f.Mode()&os.ModeSymlink != 0 {
		slog.Warn("symlink found")
		return true, nil
	}
	return false, nil
}

// Walk a directory and return all its files, which includes
// contents in nested directories. If a file or directory is
// a symbolic link, it will automatically apply rm -rf and it
// will not be part of the shred operation.
func walk(dir string) error {
	workload.directories = append(workload.directories, directory(dir))
	// Contents of the diectory to walk
	de := dirents(dir)
	// Loop over all contents in the given directory
	for _, f := range de {
		// Full relative path from executing dir
		fp := filepath.Join(dir, f.Name())

		var isDirectory bool
		var isSymbolicLink bool
		var err error

		// Cannot shred a symlink. Use normal rm -rf instead
		if isSymbolicLink, err = isSymLink(fp); err != nil {
			slog.Error("Problem reading info on path: %s.", fp, err)
		}
		if isSymbolicLink {
			slog.Info("%s is a symbolic link, removing with 'rm -rf'...")
			cmd := exec.Command("rm", "-rf", fp)
			if err := cmd.Run(); err != nil {
				slog.Error("Faile to remove symbolic link:", fp)
			}
			continue
		}

		// Recursively walk the sub directory
		if isDirectory, err = isdir(fp); err != nil {
			slog.Warn("Ignoring error and continue with dir walk.")
			continue
		}
		if isDirectory {
			workload.directories = append(workload.directories, directory(fp))
			if err := walk(fp); err != nil {
				return err
			}
			continue
		}

		// Append files to workload list
		workload.files = append(workload.files, file(fp))
	}
	return nil
}
