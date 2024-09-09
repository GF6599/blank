package main

import (
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
)

func main() {
	r, _ := walk("./cidr.xyz")
	fmt.Println(r)
	fmt.Println(len(r))

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
		fmt.Println("Failed to read path info!")
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
			return nil, err
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
