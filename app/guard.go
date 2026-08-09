package app

import (
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"
)

// systemPaths are the directories an operating system installs itself into.
// Naming one is never what the caller meant, and the list costs nothing to a
// caller who meant something narrower.
var systemPaths = []string{
	"/bin", "/boot", "/cores", "/dev", "/etc", "/home", "/lib", "/lib32",
	"/lib64", "/opt", "/proc", "/root", "/run", "/sbin", "/srv", "/sys",
	"/usr", "/var",
	"/Applications", "/Library", "/System", "/Users", "/Volumes", "/private",
}

// protectedPaths reads the home directory on each call rather than once at
// startup, so the answer belongs to the caller running the command.
func protectedPaths() []string {
	paths := slices.Clone(systemPaths)
	if home, err := os.UserHomeDir(); err == nil && home != "" {
		paths = append(paths, filepath.Clean(home))
	}
	return paths
}

// guard reports why a run refuses to destroy path. A path is refused when it is
// protected, and also when it holds a protected path, because destroying a
// directory destroys everything under it. That second rule is the one that
// works: a list of names cannot know that home lives under /mnt/nfs on this
// machine, and comparing against home itself does.
//
// There is no flag to override a refusal. The answer to one is to name a
// narrower path, which is also the answer to the mistake the guard catches.
func guard(path string) error {
	if filepath.Dir(path) == path {
		return fmt.Errorf("%s is the root of a filesystem, which blank refuses to destroy", displayText(path))
	}
	for _, protected := range protectedPaths() {
		switch {
		case path == protected:
			return fmt.Errorf("%s is a protected path, which blank refuses to destroy", displayText(path))
		case holds(path, protected):
			return fmt.Errorf("%s holds the protected path %s, so blank refuses to destroy it",
				displayText(path), displayText(protected))
		}
	}
	return nil
}

// holds reports whether inner lies under outer. Both are already absolute and
// clean, so this is a comparison of names and never a read of the filesystem.
func holds(outer, inner string) bool {
	rel, err := filepath.Rel(outer, inner)
	if err != nil || rel == "." {
		return false
	}
	return rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator))
}
