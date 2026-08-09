// Package cmd contains the root command for starting the blank application
package cmd

import (
	"context"
	"fmt"
	"io"
	"log/slog"
	"os"
	"sync"

	"blank/app"
	tui "blank/tui"

	"github.com/spf13/cobra"
)

var (
	noUI      bool
	logToFile bool
	rootCmd   = &cobra.Command{
		Use:   "blank [file|dir]...",
		Short: "Destroy files and directories beyond recovery.",
		Long: "blank overwrites every file 10 times with random data and once with zeros, " +
			"then removes the directories it walked. Deletion is permanent.",
		// A failure has to reach the exit code, so the command returns it and
		// keeps the usage text for a usage error.
		SilenceUsage: true,
		RunE: func(cmd *cobra.Command, args []string) error {
			if len(args) == 0 {
				return cmd.Help()
			}
			// The log closes when this returns, so both modes below wait for the
			// engine before they do.
			closeLog := setupLogging(logToFile, !noUI)
			defer closeLog()
			if noUI {
				return runHeadless(args)
			}
			return tui.UIStart(args)
		},
	}
)

func init() {
	rootCmd.Flags().BoolVar(&noUI, "no-ui", false, "Disable the TUI and stream progress to stdout")
	rootCmd.Flags().BoolVar(&logToFile, "log", false, "Write logs to blank.log")
	_ = rootCmd.Flags().MarkHidden("no-ui")
}

func Execute() {
	// Cobra has already written the error to stderr.
	if err := rootCmd.Execute(); err != nil {
		os.Exit(1)
	}
}

func runHeadless(args []string) error {
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

	err := app.Blank(ctx, args, progressCh, activityCh)
	wg.Wait()
	return err
}

func setupLogging(enable bool, suppress bool) func() {
	if enable {
		lf, err := os.OpenFile("blank.log", os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o644)
		if err != nil {
			if suppress {
				slog.SetDefault(slog.New(slog.NewTextHandler(io.Discard, &slog.HandlerOptions{})))
			}
			slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{})).
				Error("Failed to open log file", "ERR", err)
			return func() {}
		}
		logger := slog.New(slog.NewTextHandler(lf, &slog.HandlerOptions{}))
		slog.SetDefault(logger)
		return func() {
			if err := lf.Close(); err != nil {
				slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{})).
					Error("Failed to close the log file", "ERR", err)
			}
		}
	}
	if suppress {
		slog.SetDefault(slog.New(slog.NewTextHandler(io.Discard, &slog.HandlerOptions{})))
	}
	return func() {}
}
