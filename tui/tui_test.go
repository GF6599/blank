package tui

import (
	"context"
	"errors"
	"fmt"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/charmbracelet/bubbles/spinner"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"

	"blank/app"
)

// The activity panel keeps a bounded history.
// Given far more messages than the panel retains, when they have all arrived,
// then the model holds at most the cap and the view still shows the newest.
// Why it matters: every message re-renders every retained row, so an unbounded
// history makes the render cost of a run grow with the square of the file count,
// and it holds the path of every destroyed file in memory.
func TestActivityHistoryStaysBounded(t *testing.T) {
	// Given: a model sized for a standard terminal.
	var cur tea.Model = previewModel(t, 80, 24, 0, nil, false)

	// When: many more messages arrive than the panel retains.
	const sent = activityHistory * 3
	for i := range sent {
		cur, _ = cur.Update(activityMsg{Verb: "Overwriting", Path: fmt.Sprintf("/data/file-%06d.bin", i), Stage: app.StageStart})
	}
	got := cur.(model)

	// Then: the history is capped rather than growing with the run.
	if len(got.rows) > activityHistory {
		t.Fatalf("history grew to %d rows, the cap is %d", len(got.rows), activityHistory)
	}
	// And: the newest line is the one on screen, so the cap trims the old end.
	newest := fmt.Sprintf("file-%06d.bin", sent-1)
	if !strings.Contains(got.View(), newest) {
		t.Fatalf("newest line %q is missing from the view", newest)
	}
}

// The scroll bar reports which part of the history the panel is showing.
// Given a panel showing 8 rows of 40, when the view sits at the top, halfway and
// at the bottom, then the thumb sits at the matching point of the track.
// Why it matters: the panel shows 8 rows of a history 200 deep, so without a
// position the operator cannot tell whether the newest file is on screen or two
// hundred rows below it.
func TestScrollBarTracksWhereTheViewSits(t *testing.T) {
	// Given: a panel showing a fifth of what it holds.
	const height, total = 8, 40
	const travel = total - height

	// When: the bar is drawn at each end of the travel and halfway along it.
	top := thumbRows(t, scrollbar(height, total, 0))
	middle := thumbRows(t, scrollbar(height, total, travel/2))
	bottom := thumbRows(t, scrollbar(height, total, travel))

	// Then: the thumb reaches the first row and the last, so both ends of the
	// history read as reachable rather than as more scrolling.
	if top[0] != 0 {
		t.Fatalf("at the top the thumb starts on row %d, not the first", top[0])
	}
	if last := bottom[len(bottom)-1]; last != height-1 {
		t.Fatalf("at the bottom the thumb ends on row %d, the track ends on %d", last, height-1)
	}
	// And: it travels between them rather than jumping from one end to the other.
	if top[0] >= middle[0] || middle[0] >= bottom[0] {
		t.Fatalf("thumb did not travel: top=%d middle=%d bottom=%d", top[0], middle[0], bottom[0])
	}
}

// A panel with nothing to scroll draws no thumb and still reserves the column.
// Given fewer rows than the panel shows, when the bar renders, then no row
// carries a thumb and every row is as wide as one that does.
// Why it matters: a bar that claims its space only once it appears re-clamps
// every filename the moment the history outgrows the panel, so the whole list
// shifts sideways in the middle of a run.
func TestScrollBarReservesItsColumnWhenNothingScrolls(t *testing.T) {
	// Given / When: a panel holding less than it can show, and a full one.
	const height = 8
	quiet := scrollbar(height, 3, 0)
	busy := scrollbar(height, 40, 0)

	// Then: the track spans the panel either way, so it cannot be given back.
	if len(quiet) != height || len(busy) != height {
		t.Fatalf("track is %d rows quiet and %d rows busy, the panel shows %d",
			len(quiet), len(busy), height)
	}
	// And: it costs the same either way, so no label is re-clamped at the moment
	// the history outgrows the panel.
	for i := range quiet {
		if lipgloss.Width(quiet[i]) != lipgloss.Width(busy[i]) {
			t.Fatalf("row %d: quiet column measured %d, busy measured %d",
				i, lipgloss.Width(quiet[i]), lipgloss.Width(busy[i]))
		}
	}
	// And: nothing is lit, because there is nothing above or below to point at.
	for i, row := range quiet {
		if strings.Contains(row, scrollThumb) {
			t.Fatalf("row %d drew a thumb with nothing to scroll: %q", i, row)
		}
	}
}

// The scroll bar is drawn inside the frame, in its own column.
// Given a history deeper than the panel, when the operator scrolls to the top,
// then the thumb moves up a column that is the same one on every row.
// Why it matters: a bar composed from the row content rather than from a
// reserved column tears the frame at the widths where a filename nearly fits.
func TestActivityPanelDrawsTheScrollBarInsideTheFrame(t *testing.T) {
	// Given: a panel holding far more history than it shows.
	var cur tea.Model = previewModel(t, 80, 24, 0.5, nil, false)
	for i := range 40 {
		cur, _ = cur.Update(activityMsg{Verb: "Overwriting", Path: fmt.Sprintf("/data/file-%03d.bin", i), Stage: app.StageStart})
	}

	// When: the newest line has just arrived, so the view sits at the bottom.
	atBottom := barColumn(t, cur.(model))

	// Then: the thumb marks the end of the history.
	if last := atBottom[len(atBottom)-1]; string(last) != scrollThumb {
		t.Fatalf("bottom row of the track holds %q, not the thumb\n%s", string(last), cur.(model).View())
	}

	// When: the operator scrolls back to the start.
	for range len(atBottom) * 40 {
		cur, _ = cur.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune{'k'}})
	}
	atTop := barColumn(t, cur.(model))

	// Then: the thumb has moved to the start of the track.
	if string(atTop[0]) != scrollThumb {
		t.Fatalf("top row of the track holds %q, not the thumb\n%s", string(atTop[0]), cur.(model).View())
	}
	if string(atTop[len(atTop)-1]) == scrollThumb {
		t.Fatalf("thumb still covers the bottom of the track after scrolling up\n%s", cur.(model).View())
	}
}

// A settle marks the target it names and leaves the others alone.
// Given three targets running, when the middle one reports done and the last
// reports failed, then the first is still running and each of the other two
// carries its own outcome.
// Why it matters: the panel is the only place a failure shows while the run is
// in progress, so a settle applied to the wrong row names the wrong file as
// destroyed and clears the one that actually survived.
func TestActivityRowsSettleIndependently(t *testing.T) {
	// Given: three targets the run has reached but not finished.
	paths := []string{"/data/a.bin", "/data/b.bin", "/data/c.bin"}
	var cur tea.Model = previewModel(t, 80, 24, 0, nil, false)
	for _, p := range paths {
		cur, _ = cur.Update(activityMsg{Verb: "Overwriting", Path: p, Stage: app.StageStart})
	}

	// When: the middle one succeeds and the last one fails.
	cur, _ = cur.Update(activityMsg{Verb: "Overwriting", Path: paths[1], Stage: app.StageDone})
	cur, _ = cur.Update(activityMsg{Verb: "Overwriting", Path: paths[2], Stage: app.StageFailed})

	// Then: every row holds the stage its own target reported.
	got := cur.(model)
	want := []app.Stage{app.StageStart, app.StageDone, app.StageFailed}
	if len(got.rows) != len(want) {
		t.Fatalf("expected %d rows, got %d", len(want), len(got.rows))
	}
	for i, w := range want {
		if got.rows[i].stage != w {
			t.Fatalf("row %d (%s) is %v, expected %v", i, got.rows[i].path, got.rows[i].stage, w)
		}
	}
}

// A settle for a target the panel no longer holds changes nothing.
// Given a history that has outgrown its cap, when the oldest target settles,
// then no row changes and the panel keeps its newest rows.
// Why it matters: a long run trims a slow file's row away before it finishes,
// and a search that falls through to the nearest row would settle an unrelated
// file as destroyed.
func TestSettleForATrimmedRowIsIgnored(t *testing.T) {
	// Given: a run that has pushed the first target off the end of the history.
	var cur tea.Model = previewModel(t, 80, 24, 0, nil, false)
	first := "/data/file-000000.bin"
	for i := range activityHistory + 5 {
		cur, _ = cur.Update(activityMsg{
			Verb: "Overwriting", Path: fmt.Sprintf("/data/file-%06d.bin", i), Stage: app.StageStart,
		})
	}

	// When: that first target finally reports.
	cur, _ = cur.Update(activityMsg{Verb: "Overwriting", Path: first, Stage: app.StageDone})

	// Then: nothing settled, because the row it named is gone.
	got := cur.(model)
	for _, r := range got.rows {
		if r.stage != app.StageStart {
			t.Fatalf("row %s settled to %v on a stale event", r.path, r.stage)
		}
	}
}

// The spinner stops once no target is running.
// Given a panel whose targets have all settled, when a tick arrives, then it
// schedules no further tick.
// Why it matters: the tick repaints the whole panel ten times a second, so a
// loop that outlives the run keeps a finished program busy for as long as it
// stays open.
func TestSpinnerStopsWhenNothingIsRunning(t *testing.T) {
	// Given: one target that has started, so the animation is live.
	var cur tea.Model = previewModel(t, 80, 24, 0, nil, false)
	cur, _ = cur.Update(activityMsg{Verb: "Overwriting", Path: "/data/a.bin", Stage: app.StageStart})
	cur, tick := cur.Update(spinner.TickMsg{})
	if tick == nil {
		t.Fatal("the animation stopped while a target was still running")
	}

	// When: that target settles and the next tick arrives.
	cur, _ = cur.Update(activityMsg{Verb: "Overwriting", Path: "/data/a.bin", Stage: app.StageDone})
	_, tick = cur.Update(spinner.TickMsg{})

	// Then: nothing schedules another frame.
	if tick != nil {
		t.Fatal("the animation kept running after every target settled")
	}
}

// Each stage renders as its own cell, in a column of its own.
// Given one row per stage, when they render, then a running row shows the
// animation, a failed row shows the mark, and a done row shows neither.
// Why it matters: a stopped animation is what says the file is gone, so a
// failed row that renders like a finished one reports destruction of a file
// still sitting on disk.
func TestEachStageRendersItsOwnCell(t *testing.T) {
	// Given: a panel holding one target in each stage.
	rows := []activityRow{
		{path: "/a", text: "Overwriting /a", stage: app.StageStart},
		{path: "/b", text: "Overwriting /b", stage: app.StageDone},
		{path: "/c", text: "Overwriting /c", stage: app.StageFailed},
	}
	m := previewModel(t, 80, 24, 0.5, rows, false)

	// When / Then: the failed row is the only one carrying the mark.
	view := m.View()
	if strings.Count(view, failedMark) != 1 {
		t.Fatalf("expected exactly one failure mark, view was:\n%s", view)
	}
	// And: the running row carries a frame of the animation, which the settled
	// rows do not.
	frame := strings.TrimSpace(m.spinner.View())
	if frame == "" {
		t.Fatal("the animation rendered nothing")
	}
	if strings.Count(view, frame) != 1 {
		t.Fatalf("expected the animation on the running row alone, view was:\n%s", view)
	}
}

func thumbRows(t *testing.T, rows []string) []int {
	t.Helper()
	var out []int
	for i, row := range rows {
		if strings.Contains(row, scrollThumb) {
			out = append(out, i)
		}
	}
	if len(out) == 0 {
		t.Fatalf("no thumb anywhere in the track: %q", rows)
	}
	return out
}

// The bar occupies the cell before the frame's right edge, so reading the column
// back out of the render proves it lands there rather than inside the label.
func barColumn(t *testing.T, m model) []rune {
	t.Helper()
	var col []rune
	inPanel := false
	for _, line := range strings.Split(m.View(), "\n") {
		row := []rune(strings.TrimLeft(line, " "))
		if len(row) < 2 {
			continue
		}
		switch {
		case string(row[0]) == panelBorder.TopLeft:
			inPanel = true
		case string(row[0]) == panelBorder.BottomLeft:
			inPanel = false
		case inPanel:
			col = append(col, row[len(row)-2])
		}
	}
	if len(col) == 0 {
		t.Fatal("found no panel rows in the view")
	}
	return col
}

// The status line never claims success on a run that left plaintext on disk.
// Given a run whose progress reached the end, when the engine reports that two
// targets survived, then the line counts them and never reads "complete".
// Why it matters: progress counts targets attempted rather than targets
// destroyed, so a run that fills the bar and still leaves a readable file behind
// would otherwise announce itself as a success.
func TestStatusLineNeverClaimsSuccessWhenATargetSurvived(t *testing.T) {
	// Given: a run whose progress has reached the end.
	var cur tea.Model = previewModel(t, 80, 24, 1, nil, true)

	// When: the engine reports two survivors.
	cur, _ = cur.Update(engineDoneMsg{err: &app.IncompleteRun{Failures: []app.Failure{
		{Path: "/data/locked.txt", Err: errors.New("permission denied")},
		{Path: "/data/held.bin", Err: errors.New("permission denied")},
	}}})
	got := cur.(model).View()

	// Then: the line names how many are still there.
	if !strings.Contains(got, "2 targets survived") {
		t.Fatalf("the status line does not name the survivors:\n%s", got)
	}
	// And: it does not read as a success.
	if strings.Contains(got, "complete") {
		t.Fatalf("the status line still claims completion:\n%s", got)
	}
}

// A run that stopped early says so rather than reading as still working.
// Given a run partway through, when the engine stops with nothing to report,
// then the status line says the run was cancelled.
// Why it matters: the interface stays on screen after the engine stops, so a
// line that still reads "blanking" leaves an abandoned run looking like a
// running one.
func TestStatusLineReportsACancelledRun(t *testing.T) {
	// Given: a run partway through.
	var cur tea.Model = previewModel(t, 80, 24, 0.4, nil, false)

	// When: the engine stops without an error.
	cur, _ = cur.Update(engineDoneMsg{})

	// Then: the line reports the stop.
	if got := cur.(model).View(); !strings.Contains(got, "cancelled") {
		t.Fatalf("the status line does not report the stop:\n%s", got)
	}
}

// The engine's outcome reaches both of the things that need it.
// Given a run that ends in an error, when the outcome is delivered, then the
// interface's listener and the channel UIStart waits on both receive it.
// Why it matters: UIStart receives after the program has returned and a pending
// command can outlive the program, so a single shared channel would let
// whichever receiver won leave the other blocked forever.
func TestEngineOutcomeReachesBothReaders(t *testing.T) {
	// Given: a target that cannot be read, so the run ends in an error.
	absent := filepath.Join(t.TempDir(), "absent")
	engine := make(chan error, 1)
	done := make(chan error, 1)

	// When: the run is started and allowed to finish.
	startBlankCmd(context.Background(), app.Enumerate([]string{absent}), app.Options{Jobs: 1},
		make(chan float64), make(chan app.Event), done, engine)()

	// Then: the interface learns the run ended, and so does UIStart.
	for name, ch := range map[string]chan error{"the interface": engine, "UIStart": done} {
		select {
		case err := <-ch:
			if err == nil {
				t.Fatalf("%s received no error, so the exit code would read as success", name)
			}
		case <-time.After(5 * time.Second):
			t.Fatalf("%s never received the outcome and would block forever", name)
		}
	}
}

// Quitting a run partway through says so on the way out.
// Given a run still working, when the operator presses q, then the frame the
// program leaves behind reports the run as cancelled.
// Why it matters: quitting returns tea.Quit at once, so the engine's own
// outcome never reaches the model, and the last thing left in the scrollback
// would otherwise read as a run still in progress.
func TestQuittingMidRunLeavesACancelledFrame(t *testing.T) {
	// Given: a run partway through.
	var cur tea.Model = previewModel(t, 80, 24, 0.4, demoRows("Overwriting /data/one.bin"), false)

	// When: the operator quits.
	cur, cmd := cur.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune{'q'}})

	// Then: the program is on its way out.
	if cmd == nil {
		t.Fatal("q did not quit")
	}
	// And: the frame it leaves behind reports the run as cancelled.
	if got := cur.(model).View(); !strings.Contains(got, "cancelled") {
		t.Fatalf("the frame left behind does not report the run as cancelled:\n%s", got)
	}
}

// Quitting a finished run does not retract the result.
// Given a clean run the engine has already cleared, when the operator presses q
// to dismiss the interface, then the frame still reads as complete.
// Why it matters: q is how every run is dismissed, not only an abandoned one,
// so a quit that reported itself as a cancellation would mislabel the ordinary
// path out of the program.
func TestQuittingAFinishedRunStillReadsComplete(t *testing.T) {
	// Given: a clean run the engine has cleared.
	var cur tea.Model = previewModel(t, 80, 24, 1, nil, true)
	cur, _ = cur.Update(engineDoneMsg{})

	// When: the operator dismisses the interface.
	cur, _ = cur.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune{'q'}})

	// Then: the frame still reports the run as complete.
	got := cur.(model).View()
	if !strings.Contains(got, "blanking complete") {
		t.Fatalf("dismissing a finished run retracted the result:\n%s", got)
	}
}
