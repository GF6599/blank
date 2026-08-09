package tui

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
)

// openPickerSized starts a picker on dir and gives it one terminal size.
func openPickerSized(t *testing.T, dir string, w, h int) pickerModel {
	t.Helper()
	m := newPickerModel(dir)
	updated, _ := m.Update(tea.WindowSizeMsg{Width: w, Height: h})
	return updated.(pickerModel)
}

// Every block in the picker shares one content column.
// Given a sweep of terminal sizes, when the whole picker renders, then no row
// overruns the screen and every row of the framed listing ends in one column.
// Why it matters: the directory is cut into the top rule, and a path is longer
// than any label the run screen draws, so the width where a title nearly fits is
// reached constantly rather than never.
func TestThePickerFrameSharesOneContentColumn(t *testing.T) {
	// Given: a directory whose names are long enough to need clamping.
	dir := t.TempDir()
	for _, name := range []string{
		"a-fairly-long-ascii-file-name.txt",
		"海辺の夕日.jpg",
		"short.txt",
	} {
		write(t, filepath.Join(dir, name))
	}

	// When: the picker renders at every size, rather than at three samples.
	for w := 12; w <= 140; w++ {
		h := 12 + w%30
		m := openPickerSized(t, dir, w, h)
		view := m.View()

		// Then: it fits the rows it was given.
		if got := len(strings.Split(view, "\n")); got > h {
			t.Fatalf("terminal %dx%d: the picker rendered %d rows", w, h, got)
		}

		var frame []int
		inPanel := false
		for _, line := range strings.Split(view, "\n") {
			trimmed := strings.TrimRight(line, " ")
			if trimmed == "" {
				continue
			}
			// And: no row overruns the terminal it was drawn for.
			if got := lipgloss.Width(trimmed); got > w {
				t.Fatalf("terminal %d: row %q measured %d", w, trimmed, got)
			}
			if strings.HasPrefix(strings.TrimLeft(trimmed, " "), panelBorder.TopLeft) {
				inPanel = true
			}
			if inPanel {
				frame = append(frame, lipgloss.Width(trimmed))
			}
		}

		// And: every row of the framed listing ends in the same column.
		if len(frame) < 2 {
			t.Fatalf("terminal %d: expected a framed listing, found %d rows", w, len(frame))
		}
		for i, got := range frame {
			if got != frame[0] {
				t.Fatalf("terminal %d: frame row %d measured %d, top rule measured %d",
					w, i, got, frame[0])
			}
		}
	}
}

// The picker says where it is and how much it holds.
// Given a directory of three files, when one is marked, then the view names the
// directory, the position in the listing and the size of the marked set.
// Why it matters: the marked set is what the run destroys and it outlives the
// directory it was built in, so an operator who cannot see the count cannot tell
// a set of three from a set of four.
func TestThePickerSaysWhereItIsAndWhatIsMarked(t *testing.T) {
	// Given: a directory of three files.
	dir := t.TempDir()
	for _, name := range []string{"a.txt", "b.txt", "c.txt"} {
		write(t, filepath.Join(dir, name))
	}
	m := openPickerSized(t, dir, 100, 30)

	// Then: nothing is marked yet, and the view says which directory this is.
	if !strings.Contains(m.View(), filepath.Base(dir)) {
		t.Errorf("the view does not name %s", dir)
	}
	if !strings.Contains(m.View(), "0 marked") {
		t.Errorf("the view does not report an empty set:\n%s", m.View())
	}

	// When: one entry is marked.
	m = press(t, m, " ")

	// Then: the count follows.
	if !strings.Contains(m.View(), "1 marked") {
		t.Errorf("the view does not report the mark:\n%s", m.View())
	}
	// And: the position says where in the listing the cursor sits.
	if !strings.Contains(m.View(), "1/3") {
		t.Errorf("the view does not report the position:\n%s", m.View())
	}
}

// An applied filter says how much of the directory it is hiding.
// Given a directory of four entries with a filter matching one, when the view
// renders, then the status row carries the matched count and the whole count.
// Why it matters: an applied filter looks exactly like a small directory, so an
// operator can mark what they see, confirm, and never learn that the three
// entries the filter hid were there.
func TestAnAppliedFilterSaysWhatItIsHiding(t *testing.T) {
	// Given: four entries, only one of which matches.
	dir := t.TempDir()
	for _, name := range []string{"report.pdf", "a.txt", "b.txt", "c.txt"} {
		write(t, filepath.Join(dir, name))
	}
	m := openPickerSized(t, dir, 100, 30)
	m.list.SetFilterText("report")

	// When: the view renders.
	view := m.View()

	// Then: it names both counts, so the hidden entries are still accounted for.
	if !strings.Contains(view, "1/1 of 4") {
		t.Fatalf("the status row hides what the filter removed:\n%s", view)
	}
}

// A directory that cannot be read says so.
// Given a directory the process has no permission to list, when the picker opens
// on it, then the view carries the reason rather than an empty listing.
// Why it matters: an empty listing and an unreadable one look identical, and the
// second means the operator is looking at nothing while files are still there.
func TestThePickerReportsADirectoryItCannotRead(t *testing.T) {
	// Given: a directory holding a file, with its permissions removed.
	dir := t.TempDir()
	locked := filepath.Join(dir, "locked")
	if err := os.Mkdir(locked, 0o755); err != nil {
		t.Fatalf("make the directory: %v", err)
	}
	write(t, filepath.Join(locked, "inside.txt"))
	if err := os.Chmod(locked, 0); err != nil {
		t.Fatalf("chmod the directory: %v", err)
	}
	t.Cleanup(func() { os.Chmod(locked, 0o755) })
	if _, err := os.ReadDir(locked); err == nil {
		t.Skip("this process can read a directory with no permissions")
	}

	// When: the picker opens on it.
	m := openPickerSized(t, locked, 100, 30)

	// Then: the view carries the reason it is empty.
	if !strings.Contains(m.View(), "permission denied") {
		t.Fatalf("the view hides the read failure:\n%s", m.View())
	}
}

// A picker started on a relative path can still go up.
// Given a picker opened on ".", when the up key is pressed, then it reaches the
// parent of the working directory.
// Why it matters: filepath.Dir(".") is ".", so a picker that keeps the path it
// was given can never leave the directory it started in and the up key does
// nothing at all.
func TestAPickerStartedOnARelativePathCanGoUp(t *testing.T) {
	// Given: a picker opened on the working directory by its relative name.
	wd, err := os.Getwd()
	if err != nil {
		t.Fatalf("read the working directory: %v", err)
	}
	m := openPickerSized(t, ".", 100, 30)

	// When: the up key is pressed.
	m = press(t, m, "h")

	// Then: it is in the parent of where it started.
	if want := filepath.Dir(wd); m.dir != want {
		t.Fatalf("the picker is in %s, want %s", m.dir, want)
	}
}

func TestPickerPreviewRender(t *testing.T) {
	dir := t.TempDir()
	for _, name := range []string{
		"annual-report-2026-final.pdf",
		"notes.txt",
		"id_rsa",
		"海辺の夕日.jpg",
	} {
		write(t, filepath.Join(dir, name))
	}
	for _, name := range []string{"archive", "projects"} {
		if err := os.Mkdir(filepath.Join(dir, name), 0o755); err != nil {
			t.Fatalf("make %s: %v", name, err)
		}
	}
	if err := os.Symlink(filepath.Join(dir, "notes.txt"), filepath.Join(dir, "shortcut")); err != nil {
		t.Fatalf("make the symlink: %v", err)
	}

	for _, tc := range []struct {
		name   string
		w, h   int
		keys   []string
		filter string
	}{
		{name: "narrow-40x20", w: 40, h: 20},
		{name: "standard-80x24", w: 80, h: 24, keys: []string{" ", "j", "j", " "}},
		{name: "wide-120x40", w: 120, h: 40, keys: []string{" "}},
		{name: "typing-a-filter-80x24", w: 80, h: 24, keys: []string{"/", "r", "e"}},
		{name: "filter-applied-80x24", w: 80, h: 24, filter: "re"},
		{name: "empty-directory-80x24", w: 80, h: 24, filter: "no-such-name"},
		{name: "narrowest-12x20", w: 12, h: 20},
	} {
		m := openPickerSized(t, dir, tc.w, tc.h)
		if tc.filter != "" {
			m.list.SetFilterText(tc.filter)
		}
		m = press(t, m, tc.keys...)
		logFrame(t, tc.name, tc.w, tc.h, m.View())
	}
}
