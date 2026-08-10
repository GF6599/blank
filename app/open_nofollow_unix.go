//go:build linux || darwin

package app

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"

	"golang.org/x/sys/unix"
)

// openRegularNoFollow opens the final path component without following a
// symlink. Opening relative to the parent descriptor closes the check/open gap
// that would otherwise let a replaced directory entry redirect the write.
func openRegularNoFollow(path string, flag int, perm os.FileMode, force bool) (*os.File, error) {
	dir, base := filepath.Split(path)
	dirfd, err := unix.Open(filepath.Clean(dir), unix.O_RDONLY|unix.O_DIRECTORY|unix.O_CLOEXEC, 0)
	if err != nil {
		return nil, err
	}
	defer unix.Close(dirfd)

	open := func() (int, error) {
		return unix.Openat(dirfd, base, flag|unix.O_NOFOLLOW|unix.O_CLOEXEC|unix.O_NONBLOCK, uint32(perm.Perm()))
	}
	fd, err := open()
	if err != nil && force && (errors.Is(err, unix.EACCES) || errors.Is(err, unix.EPERM)) {
		if chmodErr := unix.Fchmodat(dirfd, base, uint32(forcedMode.Perm()), unix.AT_SYMLINK_NOFOLLOW); chmodErr != nil {
			return nil, fmt.Errorf("%w (chmod also failed: %v)", err, chmodErr)
		}
		fd, err = open()
	}
	if err != nil {
		return nil, err
	}
	if err := unix.SetNonblock(fd, false); err != nil {
		unix.Close(fd)
		return nil, err
	}
	f := os.NewFile(uintptr(fd), path)
	info, err := f.Stat()
	if err != nil {
		f.Close()
		return nil, err
	}
	if !info.Mode().IsRegular() {
		f.Close()
		return nil, fmt.Errorf("not a regular file (mode %s)", info.Mode())
	}
	return f, nil
}
