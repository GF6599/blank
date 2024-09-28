package main

// INFO:
// The whole concept of shred is not entirely secure, as overwriting data on SSD
// does not always mean that the data is actually overwritten due to
// the SSD controller managing the underlying individual cells.
// For for information, consult: https://wiki.archlinux.org/title/Securely_wipe_disk

// INFO:
// This is just a project to learn about cobra, bubble tea, goroutines
// and some basic file manipulation with go.

import (
	"gshred/cmd"

	"go.uber.org/zap"
)

var log *zap.SugaredLogger

func init() {
	logger := zap.Must(zap.NewDevelopment())
	defer logger.Sync()
	log = logger.Sugar()
}

// TODO:
// Use global logger to avoid setup each time
func main() {
	log.Info("Hello from zap logger")
	cmd.Execute()
}
