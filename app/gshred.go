package app

import (
	"fmt"
	"io/fs"
	"log"
	"os"
	"os/exec"
	"path/filepath"
	"sync"
)

// Expand the CLI args and find all the individual files.
// The files will then be concurrently shredded.
func Gshred(args []string) {
	var toShred []string
	for _, arg := range args {
		id, err := isdir(arg)
		if err != nil {
			log.Fatalln("Provided path: ", arg, " is ill formed!")
		}
		if id {
			paths, err := walk(arg)
			if err != nil {
				log.Fatalln("Failed to walk the directory: ", arg)
			}
			toShred = append(toShred, paths...)
		} else {
			toShred = append(toShred, arg)
		}
	}
	log.Printf("Number of files to shred: %d", len(toShred))
	concurrentShred(toShred)
	log.Printf("Shred finished!")
}

// Delete a directory by performing rm -rf command
func shredDir(path string) error {
	// TODO: Make this more secure?
	log.Println("Deleting dir: ", path)
	cmd := exec.Command("rm", "-rf", path)
	if err := cmd.Run(); err != nil {
		return err
	}
	return nil
}

// Shred a file by performing the shred command
func shredFile(path string) error {
	cmd := exec.Command("shred", "-uzfn", "10", path)
	if err := cmd.Run(); err != nil {
		return err
	}
	return nil
}

// Given a list of files to shred, perform the shred Command
// on files concurrently, and then perform rm -rf on any
// directories afterwards
func concurrentShred(toShred []string) {
	// Channel for reporting the overall progress.
	stats := make(chan string)
	// Semaphore to limit the amount of concurrent shreding happening.
	sema := make(chan struct{}, 100)
	// Clean up the channels
	defer close(stats)
	defer close(sema)
	// Create waitgroup for all goroutines to finish
	var wg sync.WaitGroup
	// Slice to hold all the directorys, which cannot be deleted with the shred command
	var dirToDelete []string
	// Go routine to print out the overall progress.
	go func() {
		total := len(toShred)
		var done int
		for file := range stats {
			done++
			log.Println(file)
			log.Printf("%.2f%%\n", (float64(done)/float64(total))*100)
		}
	}()
	// Loop over all the files and shred them concurrently.
	for _, file := range toShred {
		file := file
		// Folders cannot be shreded and will be treated differently
		if isdir, _ := isdir(file); isdir {
			dirToDelete = append(dirToDelete, file)
			continue
		}
		// Start a goroutine to shred the file
		wg.Add(1)
		go func() {
			sema <- struct{}{}
			defer func() {
				<-sema
				stats <- fmt.Sprintf("File: %s", file)
				wg.Done()
			}()
			if err := shredFile(file); err != nil {
				log.Fatalln("Failed to shred file: ", file)
			}
		}()
	}
	// Wait for all goroutine to finish
	wg.Wait()
	// Delete the folders after all files are shreded
	for _, dir := range dirToDelete {
		if err := shredDir(dir); err != nil {
			log.Fatalln("Failed to delete dir: ", dir)
		}
		stats <- fmt.Sprintf("Directory: %s", dir)
	}
}

// Get the contents of a directory.
func dirents(dir string) []fs.DirEntry {
	files, err := os.ReadDir(string(dir))
	if err != nil {
		fmt.Println("Failed read dir contents: ", dir)
		return nil
	}
	return files
}

// Checks whether the given path is a dir.
func isdir(path string) (bool, error) {
	f, err := os.Stat(path)
	if err != nil {
		fmt.Println("Failed to read path info!", err)
		return false, err
	}
	if f.IsDir() {
		return true, nil
	}
	return false, nil
}

// Walk a directory and return all its files, which includes
// contents in nested directories.
func walk(dir string) ([]string, error) {
	var result []string
	de := dirents(dir)
	for _, f := range de {
		ff := filepath.Join(dir, f.Name())
		d, err := isdir(ff)
		if err != nil {
			fmt.Println("Ignoring error on file: ", ff)
			continue
		}
		if d {
			w, err := walk(ff)

			if err != nil {
				return nil, err
			}
			result = append(result, w...)
		} else {
			result = append(result, ff)
		}
	}
	result = append(result, dir)
	return result, nil
}
