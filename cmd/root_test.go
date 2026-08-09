package cmd

import (
	"os"
	"testing"
)

// The log file is readable only by the user who ran the command.
// Given logging turned on, when the log file is created, then no group or other
// permission bit is set on it.
// Why it matters: the log records the full path of every file the run touched,
// so it is a manifest of what was destroyed and where it lived.
func TestLogFileIsReadableOnlyByItsOwner(t *testing.T) {
	// Given: a working directory of our own, because the log lands beside the
	// caller.
	t.Chdir(t.TempDir())

	// When: logging is turned on.
	closeLog := setupLogging(true, false)
	t.Cleanup(closeLog)

	// Then: the file carries no group or other permission.
	fi, err := os.Stat("blank.log")
	if err != nil {
		t.Fatalf("expected a log file: %v", err)
	}
	if perm := fi.Mode().Perm(); perm&0o077 != 0 {
		t.Fatalf("log file mode is %04o, expected no group or other access", perm)
	}
}
