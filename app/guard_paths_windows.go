//go:build windows

package app

import (
	"os"
	"path/filepath"
	"slices"
)

func systemPaths() []string {
	paths := []string{
		`C:\Windows`,
		`C:\Program Files`,
		`C:\Program Files (x86)`,
		`C:\ProgramData`,
	}
	for _, key := range []string{"SystemRoot", "ProgramFiles", "ProgramFiles(x86)", "ProgramData", "PUBLIC"} {
		if path := os.Getenv(key); path != "" {
			path = filepath.Clean(path)
			if !slices.Contains(paths, path) {
				paths = append(paths, path)
			}
		}
	}
	return paths
}
