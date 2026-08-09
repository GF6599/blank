package cmd

import (
	"bufio"
	"errors"
	"fmt"
	"io"
	"strings"

	"blank/app"
)

// confirm asks before anything is destroyed. Only a whole "yes" goes on: the
// answer to a question nothing can undo should not be reachable by a stray
// keystroke. A closed input reads as a refusal rather than a wait, so a command
// in a pipeline stops instead of destroying a tree nobody confirmed.
func confirm(in io.Reader, out io.Writer, plan app.Plan) (bool, error) {
	fmt.Fprintf(out, "blank will destroy %s.\n", planSummary(plan))
	fmt.Fprint(out, "This cannot be undone. Type yes to continue: ")
	answer, err := bufio.NewReader(in).ReadString('\n')
	if err != nil && !errors.Is(err, io.EOF) {
		return false, fmt.Errorf("read the answer: %w", err)
	}
	fmt.Fprintln(out)
	return strings.EqualFold(strings.TrimSpace(answer), "yes"), nil
}

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
