package cmd

import (
	"fmt"
	"io"

	"blank/app"
)

// printPlan writes every target a run would reach, one per line, in the order
// it would reach them. Long output is the point: a count alone cannot tell the
// caller that the command is aimed at the wrong directory.
func printPlan(out io.Writer, plan app.Plan) {
	for _, target := range plan.Targets() {
		fmt.Fprintf(out, "%s %s\n", target.Verb, target.Path)
	}
	fmt.Fprintf(out, "Dry run. %s. Nothing was destroyed.\n", planSummary(plan))
}

// planSummary counts what a run would destroy. The dry run and the prompt read
// the same wording, so an operator who checks with one and then runs the other
// is comparing like with like.
func planSummary(plan app.Plan) string {
	return fmt.Sprintf("%s, %s, and %s",
		counted(len(plan.Files), "file", "files"),
		counted(len(plan.Removals), "symlink or special file", "symlinks and special files"),
		counted(len(plan.Directories), "directory", "directories"))
}

func counted(n int, singular, plural string) string {
	if n == 1 {
		return fmt.Sprintf("%d %s", n, singular)
	}
	return fmt.Sprintf("%d %s", n, plural)
}
