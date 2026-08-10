//go:build !linux && !darwin && !windows

package app

import "os"

func renameNoReplace(oldpath, newpath string) error {
	if err := os.Link(oldpath, newpath); err != nil {
		return err
	}
	if err := os.Remove(oldpath); err != nil {
		_ = os.Remove(newpath)
		return err
	}
	return nil
}
