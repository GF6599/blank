// blank
//
// This is just a project to learn about cobra, bubble tea, goroutines
// and some basic file manipulation with go.
//
// The whole concept of shred is not entirely secure, as overwriting data on SSD
// does not always mean that the data is actually overwritten due to
// the SSD controller managing the underlying individual cells.
// For for information, consult: https://wiki.archlinux.org/title/Securely_wipe_disk

package main

import (
	"runtime/debug"

	"github.com/GF6599/blank/cmd"
)

// version, commit, and date are set at build time through
// -ldflags "-X main.version=...", which only a goreleaser build passes. Every
// other route leaves the defaults for resolveBuildIdentity to fill in.
var (
	version = versionUnset
	commit  = commitUnset
	date    = dateUnset
)

func main() {
	info, _ := debug.ReadBuildInfo()
	linker := buildIdentity{version: version, commit: commit, date: date}
	cmd.SetVersion(resolveBuildIdentity(linker, info).String())
	cmd.Execute()
}
