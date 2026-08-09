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
	jobs      int
	passes    int
	rootCmd   = &cobra.Command{
		Use:   "blank [file|dir]...",
		Short: "Destroy files and directories beyond recovery.",
		Long: "blank overwrites every file with random data, then once with zeros, " +
			"then removes the directories it walked. Deletion is permanent.",
		// A failure has to reach the exit code, so the command returns it and
		// keeps the usage text for a usage error.
		SilenceUsage: true,
		RunE: func(cmd *cobra.Command, args []string) error {
			if len(args) == 0 {
				return cmd.Help()
			}
			opts := app.Options{Jobs: jobs, Passes: passes}
			if err := opts.Validate(); err != nil {
				return err
			}
			// The log closes when this returns, so both modes below wait for the
			// engine before they do.
			closeLog := setupLogging(logToFile, !noUI)
			defer closeLog()
			plan := app.Enumerate(args)
			if noUI {
				return runHeadless(plan, opts)
			}
			return tui.UIStart(plan, opts)
		},
	}
)

func init() {
	rootCmd.Flags().BoolVar(&noUI, "no-ui", false, "Disable the TUI and stream progress to stdout")
	rootCmd.Flags().BoolVar(&logToFile, "log", false, "Write logs to blank.log")
	rootCmd.Flags().IntVar(&jobs, "jobs", app.DefaultJobs,
		"Targets to overwrite at the same time. Raise it on a faster disk")
	rootCmd.Flags().IntVar(&passes, "passes", app.DefaultPasses,
		"Random passes over each file before the final zero pass")
	_ = rootCmd.Flags().MarkHidden("no-ui")
}

func Execute() {
	// Cobra has already written the error to stderr.
	if err := rootCmd.Execute(); err != nil {
		os.Exit(1)
	}
}

func runHeadless(plan app.Plan, opts app.Options) error {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	progressCh := make(chan float64)
	activityCh := make(chan app.Event)
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
		// One line per target, as the run reaches it. Anything that survives is
		// named in the error, so the settle events would only repeat the list.
		for e := range activityCh {
			if e.Stage != app.StageStart {
				continue
			}
			fmt.Printf("Activity: %s %s\n", e.Verb, e.Path)
		}
	}()

	err := app.Blank(ctx, plan, opts, progressCh, activityCh)
	wg.Wait()
	return err
}

func setupLogging(enable bool, suppress bool) func() {
	if enable {
		// The log names every path the run touched, so it is a manifest of what
		// was destroyed and where it lived. Nobody else needs to read it.
		lf, err := os.OpenFile("blank.log", os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o600)
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
