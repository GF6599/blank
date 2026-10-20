package main

import (
	"fmt"
	"runtime/debug"
)

// buildIdentity is what a binary answers --version with.
type buildIdentity struct {
	version string
	commit  string
	date    string
}

// String renders the banner after the command name, which Cobra prints itself.
func (b buildIdentity) String() string {
	return fmt.Sprintf("%s (commit: %s, built: %s)", b.version, b.commit, b.date)
}

// Placeholders the linker leaves when no -X flag is passed.
const (
	versionUnset = "dev"
	commitUnset  = "none"
	dateUnset    = "unknown"

	// develVersion is what Go records when it cannot derive a module version,
	// which happens only when it cannot read the repository either.
	develVersion = "(devel)"
)

// resolveBuildIdentity fills in what the linker did not, so a binary names its
// build however it was installed.
//
// Only a goreleaser build passes -X, so every other route relies on what Go
// stamps. `go install <pkg>@<tag>` is served a zip by the module proxy: it
// carries the tag and no VCS stamp. `go build` against a readable repository
// carries a VCS stamp and a version derived from it, the tag when HEAD has one
// and a pseudo-version otherwise, and Go appends "+dirty" to that version
// itself.
//
// An extracted tarball carries no .git, so Go has no repository to read and a
// build from one leaves the defaults standing.
func resolveBuildIdentity(linker buildIdentity, info *debug.BuildInfo) buildIdentity {
	// The linker is the most specific source, so what it set wins outright.
	if linker.version != versionUnset || info == nil {
		return linker
	}

	resolved := linker
	if v := info.Main.Version; v != "" && v != develVersion {
		resolved.version = v
	}

	settings := vcsSettings(info)
	if revision := settings["vcs.revision"]; revision != "" {
		resolved.commit = revision
	}
	if when := settings["vcs.time"]; when != "" {
		resolved.date = when
	}
	return resolved
}

func vcsSettings(info *debug.BuildInfo) map[string]string {
	settings := make(map[string]string, len(info.Settings))
	for _, setting := range info.Settings {
		settings[setting.Key] = setting.Value
	}
	return settings
}
