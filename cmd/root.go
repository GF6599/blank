// Package cmd contains the root command for starting the blank application
package cmd

import (
	"context"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"log/slog"
	"os"
	"sync"

	"blank/app"
	tui "blank/tui"

	"github.com/spf13/cobra"
)

const logMode os.FileMode = 0o600

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
		// The engine reports every target, and a large tree holds far more of
		// them than there are percentages. Truncating rather than rounding keeps
		// 100% for the target that actually finishes the run.
		printed := -1
		for p := range progressCh {
			if pct := int(p * 100); pct != printed {
				printed = pct
				fmt.Printf("Progress: %d%%\n", pct)
			}
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

// The log is a manifest of what a run destroyed and where it lived, so nobody
// but the owner may read it.
//
// The mode an open applies belongs to a file it creates, so an existing log is
// narrowed through the handle rather than trusted. A symlink is refused before
// the open, because appending through one writes the manifest into a file the
// operator never named.
func openLog(path string) (*os.File, error) {
	before, err := os.Lstat(path)
	existed := err == nil
	switch {
	case existed && before.Mode()&os.ModeSymlink != 0:
		return nil, fmt.Errorf("%s is a symlink", path)
	case existed && !before.Mode().IsRegular():
		return nil, fmt.Errorf("%s is not a regular file (mode %s)", path, before.Mode())
	case !existed && !errors.Is(err, fs.ErrNotExist):
		return nil, err
	}
	lf, err := os.OpenFile(path, os.O_CREATE|os.O_WRONLY|os.O_APPEND, logMode)
	if err != nil {
		return nil, err
	}
	opened, err := lf.Stat()
	if err != nil {
		lf.Close()
		return nil, err
	}
	// The check above and the open are two calls, so the entry can be replaced
	// between them.
	if existed && !os.SameFile(before, opened) {
		lf.Close()
		return nil, fmt.Errorf("%s was replaced while it was being opened", path)
	}
	if err := lf.Chmod(logMode); err != nil {
		lf.Close()
		return nil, err
	}
	return lf, nil
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
	lf, err := openLog(path)
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
