package tui

import (
	"errors"
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"

	"blank/app"
)

// pressRun drives the run screen with one keystroke per argument, in order.
func pressRun(t *testing.T, m model, keys ...string) model {
	t.Helper()
	var cur tea.Model = m
	for _, k := range keys {
		var msg tea.KeyMsg
		switch k {
		case "enter":
			msg = tea.KeyMsg{Type: tea.KeyEnter}
		case "esc":
			msg = tea.KeyMsg{Type: tea.KeyEsc}
		case "ctrl+c":
			msg = tea.KeyMsg{Type: tea.KeyCtrlC}
		default:
			msg = tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune(k)}
		}
		cur, _ = cur.Update(msg)
	}
	return cur.(model)
}

func logRows() []activityRow {
	return demoRows(
		"Overwriting /home/dev/keys/id_rsa",
		"Overwriting /home/dev/photos/beach.jpg",
		"Overwriting /home/dev/keys/id_ed25519",
	)
}

// A filter narrows the panel to the rows that match it.
// Given a run whose log holds three targets, when a filter naming one directory
// is applied, then only the rows under that directory are drawn.
// Why it matters: the panel holds 200 rows of a run that touches thousands, so
// finding the one target an operator cares about means reading every row that
// scrolls past unless they can narrow it.
func TestFilteringTheLogHidesRowsThatDoNotMatch(t *testing.T) {
	// Given: a run whose log holds targets in two directories.
	m := previewModel(t, 80, 24, 0.5, logRows(), false)

	// When: a filter naming one of them is typed and applied.
	m = pressRun(t, m, "/", "k", "e", "y", "s", "enter")

	// Then: the panel draws the matching rows and no others.
	view := m.View()
	for _, want := range []string{"id_rsa", "id_ed25519"} {
		if !strings.Contains(view, want) {
			t.Errorf("the filter hid %s, which matches it:\n%s", want, view)
		}
	}
	if strings.Contains(view, "beach.jpg") {
		t.Errorf("the filter kept beach.jpg, which does not match it:\n%s", view)
	}
}

// The filter says how much of the log it is hiding.
// Given a filter matching two rows of three, when the panel renders, then its
// title carries the filter and both counts.
// Why it matters: a filtered panel looks exactly like a short log, so an
// operator can read it as the whole run and never learn what it left out.
func TestTheLogFilterSaysHowMuchItIsHiding(t *testing.T) {
	// Given: a filter matching two rows of three.
	m := previewModel(t, 80, 24, 0.5, logRows(), false)

	// When: it is applied.
	m = pressRun(t, m, "/", "k", "e", "y", "s", "enter")

	// Then: the panel title names the filter and what it left out.
	if view := m.View(); !strings.Contains(view, "keys") || !strings.Contains(view, "2 of 3") {
		t.Fatalf("the panel does not say what the filter hid:\n%s", view)
	}
}

// The bracket pair marks the target in flight, not the last row drawn.
// Given a filter that hides the newest target, when the panel renders, then no
// row carries the pair.
// Why it matters: the pair means the run is working on this one, so putting it
// on whatever row the filter happened to leave last points it at a target that
// finished a thousand rows ago.
func TestTheMarkerFollowsTheRunAndNotTheFilter(t *testing.T) {
	// Given: a log whose newest row is the one the filter hides.
	rows := demoRows(
		"Overwriting /home/dev/keys/id_rsa",
		"Overwriting /home/dev/photos/beach.jpg",
	)
	m := previewModel(t, 80, 24, 0.5, rows, false)

	// When: a filter matching only the older row is applied.
	m = pressRun(t, m, "/", "k", "e", "y", "s", "enter")

	// Then: the row it left carries no pair, because it is not the one running.
	for _, line := range strings.Split(m.View(), "\n") {
		if strings.Contains(line, "id_rsa") && strings.Contains(line, "[") {
			t.Fatalf("the marker moved to a row that is not the newest: %q", line)
		}
	}
}

// A row settles while it is filtered out of the panel.
// Given a filter that hides a target, when that target fails, then clearing the
// filter shows it carrying the failed mark.
// Why it matters: the filter is a view of the log and not the log, so a settle
// dropped while a row is hidden would leave a destroyed target reading as still
// running for the rest of the run.
func TestARowThatSettlesWhileFilteredStillSettles(t *testing.T) {
	// Given: a filter that hides the photos directory.
	m := previewModel(t, 80, 24, 0.5, logRows(), false)
	m = pressRun(t, m, "/", "k", "e", "y", "s", "enter")

	// When: the hidden target fails.
	var cur tea.Model = m
	cur, _ = cur.Update(activityMsg{
		Verb:  "Overwriting",
		Path:  "Overwriting /home/dev/photos/beach.jpg",
		Stage: app.StageFailed,
	})

	// Then: clearing the filter shows it settled rather than still running.
	m = pressRun(t, cur.(model), "esc")
	for _, line := range strings.Split(m.View(), "\n") {
		if strings.Contains(line, "beach.jpg") && !strings.Contains(line, failedMark) {
			t.Fatalf("the settle was dropped while the row was hidden: %q", line)
		}
	}
}

// A filter never changes the verdict.
// Given a run that failed with two survivors, when a filter hides both of their
// rows, then the status line still counts two.
// Why it matters: the exit code and the status line are built from the engine's
// own error, and a count taken from what is on screen would report a clean run
// to anyone who happened to be filtering.
func TestAFilteredPanelStillReportsEveryFailure(t *testing.T) {
	// Given: a run the engine reported two survivors for.
	survived := &app.IncompleteRun{Failures: []app.Failure{
		{Path: "/home/dev/keys/id_rsa", Err: errors.New("permission denied")},
		{Path: "/home/dev/keys", Err: errors.New("directory not empty")},
	}}
	var cur tea.Model = previewModel(t, 80, 24, 1, logRows(), true)
	cur, _ = cur.Update(engineDoneMsg{err: survived})

	// When: a filter hides every row that failed.
	m := pressRun(t, cur.(model), "/", "z", "z", "z", "enter")

	// Then: the status line still names both.
	if view := m.View(); !strings.Contains(view, "2 targets survived") {
		t.Fatalf("the filter changed the verdict:\n%s", view)
	}
}

// Ctrl+C stops the run even while the filter is open.
// Given a filter half typed, when ctrl+c is pressed, then the run is cancelled.
// Why it matters: this screen is destroying files, so the key an operator
// reaches for to stop it must never be swallowed by a text input.
func TestCtrlCStopsTheRunWhileTheLogFilterIsOpen(t *testing.T) {
	// Given: a filter being typed over a running screen.
	m := previewModel(t, 80, 24, 0.5, logRows(), false)
	m = pressRun(t, m, "/", "k", "e")

	// When: ctrl+c is pressed.
	m = pressRun(t, m, "ctrl+c")

	// Then: the run is cancelled rather than the key becoming filter text.
	if !m.aborted {
		t.Fatal("ctrl+c was swallowed by the filter input")
	}
}
