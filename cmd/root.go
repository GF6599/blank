// Package cmd contains the root command for starting the blank application
package cmd

import (
	"context"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"os"
	"os/signal"
	"path/filepath"
	"sync"

	"github.com/GF6599/blank/app"
	"github.com/GF6599/blank/tui"

	"github.com/spf13/cobra"
)

const logMode os.FileMode = 0o600

// NewRootCommand builds an independent command and flag set. Keeping the state
// in this constructor lets tests and embedders run more than one command in a
// process without values or loggers leaking between invocations.
func NewRootCommand() *cobra.Command {
	var noUI, dryRun, assumeYes bool
	var logPath string
	var jobs, passes int
	rootCmd := &cobra.Command{
		Use:     "blank [file|dir]...",
		Version: versionString,
		Short:   "Destroy files and directories beyond recovery.",
		Long: "blank overwrites every file with random data, then once with zeros, " +
			"then removes the directories it walked. Deletion is permanent.",
		// A failure has to reach the exit code, so the command returns it and
		// keeps the usage text for a usage error.
		SilenceUsage: true,
		RunE: func(cmd *cobra.Command, args []string) error {
			opts := app.Options{Jobs: jobs, Passes: passes}
			if err := opts.Validate(); err != nil {
				return err
			}
			// Before the enumeration, because a directory the walk cannot read
			// is logged the moment it is found and the interface is about to
			// draw over whatever that printed.
			if !noUI {
				defer quietLogger()()
			}
			if len(args) == 0 {
				wd, wdErr := os.Getwd()
				if wdErr != nil {
					return wdErr
				}
				start, startErr := selectorStart(interactive(cmd.InOrStdin()), noUI, wd)
				if errors.Is(startErr, errShowUsage) {
					return cmd.Help()
				}
				if startErr != nil {
					return startErr
				}
				picked, pickErr := tui.PickTargets(start)
				if pickErr != nil {
					return pickErr
				}
				if len(picked) == 0 {
					// The wording the refused confirmation uses, because both
					// are the operator declining to destroy anything.
					return errors.New("cancelled, nothing was destroyed")
				}
				args = picked
			}
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
			if logPath != "" && plan.Contains(logPath) {
				return fmt.Errorf("the log path %q is inside the selected targets; choose a path the run will not destroy",
					logPath)
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
			// Opening the manifest is the last step before the engine starts. A dry
			// run and a refused confirmation therefore make no filesystem changes.
			if logPath != "" {
				closeLog, err := openManifest(logPath)
				if err != nil {
					return err
				}
				defer closeLog()
			}
			if noUI {
				ctx, stop := signal.NotifyContext(cmd.Context(), os.Interrupt)
				defer stop()
				return runHeadless(ctx, cmd.OutOrStdout(), plan, opts)
			}
			return tui.UIStart(plan, opts)
		},
	}
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
	return rootCmd
}

// versionString is what Cobra renders for --version. Package main wires it from
// the linker values before the command is built, because -X can only write to a
// variable in the main package.
var versionString = "dev"

// SetVersion supplies the build identity. It runs before NewRootCommand,
// because the command reads the value at construction.
func SetVersion(v string) { versionString = v }

// errShowUsage says the caller asked a usage question rather than named a
// place to browse.
var errShowUsage = errors.New("no targets were named and no terminal can show a selector")

// selectorStart reports the directory a run with no arguments should browse.
//
// It refuses a directory the guard already protects. Marking a row costs one
// keystroke, so a selector opened on the home directory puts the whole of it one
// key and one confirmation from destruction, and the command line requires a
// path to be typed on purpose.
func selectorStart(hasTerminal, headless bool, wd string) (string, error) {
	// A selector drawn into a pipe is a program waiting for keys that nobody
	// can send, so no terminal is a usage question and not a refusal.
	if headless || !hasTerminal {
		return "", errShowUsage
	}
	// The guard leaves the last component of a path alone, because destroying a
	// symlink unlinks the link and never its target. Browsing is the opposite:
	// a selector opened on a link lists what the link points at, so the name is
	// resolved the whole way before it is checked and after it is checked.
	if resolved, err := filepath.EvalSymlinks(wd); err == nil {
		wd = resolved
	}
	if err := app.NewGuard().Refuses(wd); err != nil {
		return "", fmt.Errorf("%w, so it will not open a selector there either. "+
			"Name a path on the command line, or change to a narrower directory first", err)
	}
	return wd, nil
}

func interactive(in io.Reader) bool {
	f, ok := in.(*os.File)
	if !ok {
		return false
	}
	info, err := f.Stat()
	return err == nil && info.Mode()&os.ModeCharDevice != 0
}

func Execute() {
	// Cobra has already written the error to stderr.
	if err := NewRootCommand().Execute(); err != nil {
		os.Exit(1)
	}
}

func runHeadless(ctx context.Context, out io.Writer, plan app.Plan, opts app.Options) error {
	progressCh := make(chan float64)
	activityCh := make(chan app.Event)
	var wg sync.WaitGroup
	var outMu sync.Mutex
	write := func(format string, args ...any) {
		outMu.Lock()
		defer outMu.Unlock()
		fmt.Fprintf(out, format, args...)
	}
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
				write("Progress: %d%%\n", pct)
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
			write("Activity: %s %s\n", e.Verb, e.Path)
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
	lf, err := app.OpenLogFile(path, logMode)
	if err != nil {
		return nil, err
	}
	if err := lf.Chmod(logMode); err != nil {
		lf.Close()
		return nil, err
	}
	return lf, nil
}

// quietLogger silences the default logger and returns the restore. The interface
// owns the terminal, so anything logged while it is up belongs in the manifest or
// nowhere at all.
func quietLogger() func() {
	previous := slog.Default()
	slog.SetDefault(slog.New(slog.NewTextHandler(io.Discard, &slog.HandlerOptions{})))
	return func() { slog.SetDefault(previous) }
}

// openManifest points the default logger at path and returns the restore. A path
// that cannot be opened is returned as an error rather than warned about,
// because a run that destroys the tree and the record of it together leaves the
// caller nothing to read afterwards.
func openManifest(path string) (func(), error) {
	lf, err := openLog(path)
	if err != nil {
		return nil, fmt.Errorf("open the log file: %w", err)
	}
	previous := slog.Default()
	slog.SetDefault(slog.New(slog.NewTextHandler(lf, &slog.HandlerOptions{})))
	return func() {
		slog.SetDefault(previous)
		if err := lf.Close(); err != nil {
			slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{})).
				Error("Failed to close the log file", "ERR", err)
		}
	}, nil
}
