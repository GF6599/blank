package app

import (
	"fmt"
	"os"
	"os/user"
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

// A test drives the guard through this rather than naming the real home.
var currentUser = user.Current

// protectedPaths reads the home directory from the environment and from the
// user database, and protects whichever of the two it gets. HOME belongs to
// whoever started the process: missing, it drops the protection, and wrong, it
// aims the protection at a directory nobody lives in. The user database is the
// answer the environment cannot move. The environment still counts as well,
// because a caller who sets HOME on purpose means the directory they named.
func protectedPaths() []string {
	paths := slices.Clone(systemPaths)
	if home, err := os.UserHomeDir(); err == nil && home != "" {
		paths = append(paths, filepath.Clean(home))
	}
	if u, err := currentUser(); err == nil && u.HomeDir != "" {
		paths = append(paths, filepath.Clean(u.HomeDir))
	}
	// /etc is a symlink to /private/etc on macOS, and a home directory is often
	// a symlink to wherever the disk holding it is mounted. Both names reach one
	// directory, so both have to be on the list.
	for _, p := range slices.Clone(paths) {
		if real, err := filepath.EvalSymlinks(p); err == nil && !slices.Contains(paths, real) {
			paths = append(paths, real)
		}
	}
	return paths
}

// The last component is left as it was: a symlink argument is unlinked rather
// than followed, so resolving it would refuse a link whose target the run never
// opens.
func realPath(path string) string {
	dir, base := filepath.Split(path)
	real, err := filepath.EvalSymlinks(filepath.Clean(dir))
	if err != nil {
		return path
	}
	return filepath.Join(real, base)
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
	protected := protectedPaths()
	if err := refuse(path, protected); err != nil {
		return err
	}
	real := realPath(path)
	if real == path {
		return nil
	}
	if err := refuse(real, protected); err != nil {
		return fmt.Errorf("%s resolves to %s: %w", displayText(path), displayText(real), err)
	}
	return nil
}

func refuse(path string, protected []string) error {
	if filepath.Dir(path) == path {
		return fmt.Errorf("%s is the root of a filesystem, which blank refuses to destroy", displayText(path))
	}
	for _, p := range protected {
		switch {
		case path == p:
			return fmt.Errorf("%s is a protected path, which blank refuses to destroy", displayText(path))
		case holds(path, p):
			return fmt.Errorf("%s holds the protected path %s, so blank refuses to destroy it",
				displayText(path), displayText(p))
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
