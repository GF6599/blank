//go:build !windows

package app

// systemPaths are the directories an operating system installs itself into.
// Naming one is never what the caller meant, and the list costs nothing to a
// caller who meant something narrower.
func systemPaths() []string {
	return []string{
		"/bin", "/boot", "/cores", "/dev", "/etc", "/home", "/lib", "/lib32",
		"/lib64", "/opt", "/proc", "/root", "/run", "/sbin", "/srv", "/sys",
		"/usr", "/var",
		"/Applications", "/Library", "/System", "/Users", "/Volumes", "/private",
	}
}
