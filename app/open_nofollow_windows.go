//go:build windows

package app

import (
	"fmt"
	"os"

	"golang.org/x/sys/windows"
)

func openRegularNoFollow(path string, flag int, perm os.FileMode, force bool) (*os.File, error) {
	pathp, err := windows.UTF16PtrFromString(path)
	if err != nil {
		return nil, err
	}
	access := uint32(windows.GENERIC_WRITE)
	if flag&os.O_APPEND != 0 {
		access = windows.FILE_APPEND_DATA
	}
	disposition := uint32(windows.OPEN_EXISTING)
	if flag&os.O_CREATE != 0 {
		disposition = windows.OPEN_ALWAYS
	}
	open := func() (windows.Handle, error) {
		return windows.CreateFile(pathp, access,
			windows.FILE_SHARE_READ|windows.FILE_SHARE_WRITE|windows.FILE_SHARE_DELETE,
			nil, disposition, windows.FILE_ATTRIBUTE_NORMAL|windows.FILE_FLAG_OPEN_REPARSE_POINT, 0)
	}
	h, err := open()
	if err != nil && force && err == windows.ERROR_ACCESS_DENIED {
		attrs, attrErr := windows.GetFileAttributes(pathp)
		if attrErr != nil {
			return nil, err
		}
		if attrs&windows.FILE_ATTRIBUTE_REPARSE_POINT != 0 {
			return nil, fmt.Errorf("path is a symlink or reparse point")
		}
		if attrErr = windows.SetFileAttributes(pathp, attrs&^windows.FILE_ATTRIBUTE_READONLY); attrErr != nil {
			return nil, fmt.Errorf("%w (chmod also failed: %v)", err, attrErr)
		}
		h, err = open()
	}
	if err != nil {
		return nil, err
	}
	var handleInfo windows.ByHandleFileInformation
	if err := windows.GetFileInformationByHandle(h, &handleInfo); err != nil {
		windows.CloseHandle(h)
		return nil, err
	}
	if handleInfo.FileAttributes&windows.FILE_ATTRIBUTE_REPARSE_POINT != 0 {
		windows.CloseHandle(h)
		return nil, fmt.Errorf("path is a symlink or reparse point")
	}
	f := os.NewFile(uintptr(h), path)
	info, err := f.Stat()
	if err != nil {
		f.Close()
		return nil, err
	}
	if !info.Mode().IsRegular() || info.Mode()&os.ModeSymlink != 0 {
		f.Close()
		return nil, fmt.Errorf("not a regular file (mode %s)", info.Mode())
	}
	_ = perm
	return f, nil
}
