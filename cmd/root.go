// Package cmd contains the root command for starting the gs application
package cmd

import (
	"context"
	"fmt"
	"os"
	"sync"

	"gshred/app"
	tui "gshred/tui"

	"github.com/spf13/cobra"
)

var (
	noUI    bool
	rootCmd = &cobra.Command{
		Use:   "gshred [file|dir]...",
		Short: "Concurrnet Shred command implemented in go.",
		Long:  "Will use the bash 'shred -n 10 -uvz' command to shred any files or dir provided to gshred.",
		Run: func(cmd *cobra.Command, args []string) {
			if len(args) == 0 {
				cmd.Help()
				return
			}
			if noUI {
				runHeadless(args)
				return
			}
			tui.UIStart(args)
		},
	}
)

func init() {
	rootCmd.Flags().BoolVar(&noUI, "no-ui", false, "Disable the TUI and stream progress to stdout")
	_ = rootCmd.Flags().MarkHidden("no-ui")
}

func Execute() {
	if err := rootCmd.Execute(); err != nil {
		fmt.Println(err)
		os.Exit(1)
	}
}

func runHeadless(args []string) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	progressCh := make(chan float64)
	activityCh := make(chan string)
	var wg sync.WaitGroup
	wg.Add(2)

	go func() {
		defer wg.Done()
		for p := range progressCh {
			fmt.Printf("Progress: %.0f%%\n", p*100)
		}
	}()

	go func() {
		defer wg.Done()
		for msg := range activityCh {
			fmt.Printf("Activity: %s\n", msg)
		}
	}()

	app.Gshred(ctx, args, progressCh, activityCh)
	wg.Wait()
}
