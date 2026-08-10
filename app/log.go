package app

import "os"

// OpenLogFile opens a manifest for append without following its final path
// component. It lives beside the deletion open because both operations require
// the same platform-specific no-follow guarantees.
func OpenLogFile(path string, mode os.FileMode) (*os.File, error) {
	return openRegularNoFollow(path, os.O_CREATE|os.O_WRONLY|os.O_APPEND, mode, false)
}
