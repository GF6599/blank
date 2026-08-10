//go:build darwin

package app

import (
	"os"

	"golang.org/x/sys/unix"
)

func renameNoReplace(oldpath, newpath string) error {
	if err := unix.RenameatxNp(unix.AT_FDCWD, oldpath, unix.AT_FDCWD, newpath, unix.RENAME_EXCL); err != nil {
		return &os.LinkError{Op: "rename", Old: oldpath, New: newpath, Err: err}
	}
	return nil
}
