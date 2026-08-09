package tui

import (
	"fmt"
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
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
		cur, _ = cur.Update(activityMsg(fmt.Sprintf("Overwriting /data/file-%06d.bin", i)))
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
