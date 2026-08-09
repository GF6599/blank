package tui

import (
	"fmt"
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
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
		cur, _ = cur.Update(activityMsg{Verb: "Overwriting", Path: fmt.Sprintf("/data/file-%06d.bin", i)})
	}
	got := cur.(model)

	// Then: the history is capped rather than growing with the run.
	if len(got.logs) > activityHistory {
		t.Fatalf("history grew to %d rows, the cap is %d", len(got.logs), activityHistory)
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
		cur, _ = cur.Update(activityMsg{Verb: "Overwriting", Path: fmt.Sprintf("/data/file-%03d.bin", i)})
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
