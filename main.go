package main

import (
	"fmt"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"sync"
)

var dir = "./langchain"

func main() {
	r, _ := walk(dir)
	stats := make(chan bool)
	sema := make(chan struct{}, 100)
	var wg sync.WaitGroup

	go func() {
		total := len(r)
		var done int
		for range stats {
			done++
			fmt.Printf("%.2f%%\n", (float64(done)/float64(total))*100)
		}
	}()

	for _, file := range r {
		file := file
		wg.Add(1)
		go func() {
			sema <- struct{}{}
			defer func() {
				<-sema
				stats <- true
				wg.Done()
			}()
			cmd := exec.Command("shred", "-uzfn", "10", file)
			_, err := cmd.Output()

			if err != nil {
				fmt.Println("cmd error: ", file, err)
				return
			}
		}()
	}

	wg.Wait()
	close(stats)
	close(sema)
	cmd := exec.Command("rm", "-rf", dir)
	cmd.Run()

}

func dirents(dir string) []fs.DirEntry {
	files, err := os.ReadDir(string(dir))
	if err != nil {
		fmt.Println("Failed read dir contents: ", dir)
		return nil
	}
	return files
}

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
	return result, nil
}
