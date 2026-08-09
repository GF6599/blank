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

import "blank/cmd"

func main() {
	cmd.Execute()
}
