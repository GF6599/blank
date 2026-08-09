// Package cmd contains the root command for starting the blank application
package cmd

import (
	"context"
	"errors"
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
	logPath   string
	dryRun    bool
	assumeYes bool
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
			closeLog, err := setupLogging(logPath, !noUI)
			if err != nil {
				return err
			}
			defer closeLog()
			plan, err := app.Enumerate(args)
			if err != nil {
				return err
			}
			if dryRun {
				printPlan(cmd.OutOrStdout(), plan)
				// A plan built over a directory nobody could read is missing
				// whatever is inside it, so reporting it as clean would
				// understate what the run would destroy.
				return plan.Err()
			}
			// Nothing to destroy is not worth a question, and the enumeration
			// failures below still reach the caller.
			if !assumeYes && plan.Total() > 0 {
				ok, err := confirm(cmd.InOrStdin(), cmd.OutOrStdout(), plan)
				if err != nil {
					return err
				}
				if !ok {
					// An error rather than a clean exit, because exit zero is
					// what this command says when everything was destroyed.
					return errors.New("cancelled, nothing was destroyed")
				}
			}
			if noUI {
				return runHeadless(plan, opts)
			}
			return tui.UIStart(plan, opts)
		},
	}
)

func init() {
	rootCmd.Flags().BoolVar(&noUI, "no-ui", false,
		"Turn off the terminal interface and print one line per target to stdout")
	rootCmd.Flags().StringVar(&logPath, "log", "",
		"Write the run log to this path. It names every file destroyed, so it is a manifest of what was on the disk")
	rootCmd.Flags().BoolVar(&dryRun, "dry-run", false, "List what would be destroyed and destroy nothing")
	rootCmd.Flags().BoolVarP(&assumeYes, "yes", "y", false, "Destroy without asking first")
	rootCmd.Flags().IntVar(&jobs, "jobs", app.DefaultJobs,
		"Targets to overwrite at the same time. Raise it on a faster disk")
	rootCmd.Flags().IntVar(&passes, "passes", app.DefaultPasses,
		"Random passes over each file before the final zero pass")
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

// setupLogging points the default logger at path, and discards it entirely when
// no path was given and the interface owns the terminal. A path that cannot be
// opened is returned as an error rather than warned about, because a run that
// destroys the tree and the record of it together leaves the caller nothing to
// read afterwards.
func setupLogging(path string, suppress bool) (func(), error) {
	if path == "" {
		if suppress {
			slog.SetDefault(slog.New(slog.NewTextHandler(io.Discard, &slog.HandlerOptions{})))
		}
		return func() {}, nil
	}
	// The log names every path the run touched, so it is a manifest of what was
	// destroyed and where it lived. Nobody else needs to read it.
	lf, err := os.OpenFile(path, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o600)
	if err != nil {
		return nil, fmt.Errorf("open the log file: %w", err)
	}
	slog.SetDefault(slog.New(slog.NewTextHandler(lf, &slog.HandlerOptions{})))
	return func() {
		if err := lf.Close(); err != nil {
			slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{})).
				Error("Failed to close the log file", "ERR", err)
		}
	}, nil
}
