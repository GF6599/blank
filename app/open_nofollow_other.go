//go:build !linux && !darwin && !windows

package app

import (
	"fmt"
	"os"
)

func openRegularNoFollow(path string, flag int, perm os.FileMode, force bool) (*os.File, error) {
	before, err := os.Lstat(path)
	if err != nil && flag&os.O_CREATE == 0 {
		return nil, err
	}
	if err == nil && !before.Mode().IsRegular() {
		return nil, fmt.Errorf("not a regular file (mode %s)", before.Mode())
	}
	f, err := os.OpenFile(path, flag, perm)
	if os.IsPermission(err) && force {
		if chmodErr := os.Chmod(path, forcedMode); chmodErr != nil {
			return nil, fmt.Errorf("%w (chmod also failed: %v)", err, chmodErr)
		}
		f, err = os.OpenFile(path, flag, perm)
	}
	if err != nil {
		return nil, err
	}
	after, err := f.Stat()
	if err != nil || !after.Mode().IsRegular() || (before != nil && !os.SameFile(before, after)) {
		f.Close()
		if err != nil {
			return nil, err
		}
		return nil, fmt.Errorf("path changed while it was being opened")
	}
	return f, nil
}
