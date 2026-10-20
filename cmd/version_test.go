package cmd

import (
	"bytes"
	"strings"
	"testing"
)

func TestTheVersionFlagReportsTheBuildAndDestroysNothing(t *testing.T) {
	previous := versionString
	t.Cleanup(func() { versionString = previous })
	versionString = "v1.2.3 (commit: abc1234, built: 2026-08-29T00:00:00Z)"

	var out bytes.Buffer
	cmd := NewRootCommand()
	cmd.SetOut(&out)
	cmd.SetArgs([]string{"--version"})
	if err := cmd.Execute(); err != nil {
		t.Fatalf("running --version: %v", err)
	}
	if !strings.Contains(out.String(), versionString) {
		t.Fatalf("--version printed:\n%s", out.String())
	}
}
