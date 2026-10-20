package tui

import (
	"path/filepath"
	"strings"
	"testing"

	"github.com/GF6599/blank/app"
)

// A path the run would refuse cannot be marked.
// Given the picker at the root of the filesystem, when space is pressed on a
// system directory, then nothing is marked and the view says why.
// Why it matters: the guard refuses the whole command line, so one protected
// path marked among nine ordinary ones destroys none of them, and the operator
// learns that only after browsing, marking and confirming.
func TestAProtectedPathCannotBeMarked(t *testing.T) {
	// Given: the picker at the root of the filesystem, on /etc.
	m := openPickerSized(t, "/", 100, 30)
	m = cursorOn(t, m, "etc")

	// When: space is pressed on it.
	m = press(t, m, " ")

	// Then: nothing was marked.
	if len(m.marked) != 0 {
		t.Fatalf("/etc was marked: %v", m.marked)
	}
	// And: the view says why rather than leaving the key looking broken.
	if view := m.View(); !strings.Contains(view, "protected path") {
		t.Fatalf("the view gives no reason for the refusal:\n%s", view)
	}
}

// The row itself says a path cannot be marked.
// Given a listing holding a protected path beside an ordinary one, when the rows
// render, then their state cells differ.
// Why it matters: an operator reads the list before pressing anything, and a
// refusal that only appears after a keystroke makes them find each protected
// path by trying it.
func TestAProtectedRowReadsAsRefusedBeforeItIsPressed(t *testing.T) {
	// Given: the root of the filesystem, which holds protected paths.
	m := openPickerSized(t, "/", 100, 30)
	entries := make([]entry, 0, len(m.list.Items()))
	for _, item := range m.list.Items() {
		entries = append(entries, item.(entry))
	}
	rows := renderRows(t, 100, entries, m.marked, 0)

	// Then: the row for a protected path carries the refused cell.
	var found bool
	for i, e := range entries {
		if e.name != "etc" {
			continue
		}
		found = true
		if !strings.Contains(rows[i], refusedMark) {
			t.Fatalf("the row for /etc is not marked as refused: %q", rows[i])
		}
	}
	if !found {
		t.Skip("this filesystem has no /etc to refuse")
	}
}

// The selector never climbs into a directory the run protects.
// Given a picker in a temporary tree, when the up key is pressed far more times
// than the tree is deep, then it stops in a directory the run would accept, and
// it says why it stopped.
// Why it matters: refusing to open the selector in a protected directory buys
// nothing if the up key walks into one, and every temporary directory on macOS
// sits under /var, so the boundary is two or three presses away.
func TestTheSelectorNeverClimbsIntoAProtectedDirectory(t *testing.T) {
	// Given: a picker inside a temporary tree.
	m := openPickerSized(t, t.TempDir(), 100, 30)

	// When: the up key is pressed far past the root of that tree.
	for range 40 {
		m = press(t, m, "h")
	}

	// Then: it is somewhere the run would accept.
	if err := app.NewGuard().Refuses(m.dir); err != nil {
		t.Fatalf("the selector climbed into %s: %v", m.dir, err)
	}
	// And: the last press said why it went no further.
	if view := m.View(); !strings.Contains(view, "protected") && !strings.Contains(view, "root of a filesystem") {
		t.Fatalf("the selector stopped with no reason on screen:\n%s", view)
	}
}

// An ordinary path is still markable.
// Given a temporary directory holding one file, when space is pressed on it,
// then it is marked.
// Why it matters: a refusal rule that catches everything is the same as a picker
// that does not work, and it would pass the test above.
func TestAnOrdinaryPathIsStillMarkable(t *testing.T) {
	// Given: a file nobody protects.
	dir := t.TempDir()
	target := filepath.Join(dir, "notes.txt")
	write(t, target)
	m := openPickerSized(t, dir, 100, 30)

	// When: space is pressed on it.
	m = press(t, m, " ")

	// Then: it is marked.
	if !m.marked[target] {
		t.Fatalf("%s was refused: %v", target, m.marked)
	}
}
