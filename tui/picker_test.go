package tui

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/charmbracelet/lipgloss"
)

func entriesIn(t *testing.T, dir string) []entry {
	t.Helper()
	found, err := os.ReadDir(dir)
	if err != nil {
		t.Fatalf("read %s: %v", dir, err)
	}
	out := make([]entry, len(found))
	for i, de := range found {
		out[i] = newEntry(dir, de)
	}
	return out
}

func renderRows(t *testing.T, width int, entries []entry, marked map[string]bool, cursor int) []string {
	t.Helper()
	d := entryDelegate{marked: marked}
	l := newPickerList(marked)
	l.SetSize(width, len(entries)+10)
	l.SetItems(toItems(entries))
	l.Select(cursor)
	rows := make([]string, len(entries))
	for i := range entries {
		var b strings.Builder
		d.Render(&b, l, i, entries[i])
		rows[i] = b.String()
	}
	return rows
}

// A marked entry reads differently from one that is not marked.
// Given two entries where only one is marked, when the rows render, then the
// marked row carries the mark and both rows measure the same width.
// Why it matters: the mark is the only record of what the picker will hand to a
// run that cannot be undone, and a mark that changes a row's width shifts every
// label sideways as the set is built.
func TestAMarkedEntryCarriesItsMark(t *testing.T) {
	// Given: a directory holding two files, one of them marked.
	dir := t.TempDir()
	write(t, filepath.Join(dir, "keep.txt"))
	write(t, filepath.Join(dir, "shred.txt"))
	entries := entriesIn(t, dir)
	marked := map[string]bool{filepath.Join(dir, "shred.txt"): true}

	// When: both rows render at one width.
	rows := renderRows(t, 40, entries, marked, 0)

	// Then: only the marked row carries the mark.
	if strings.Contains(rows[0], markedMark) {
		t.Errorf("the unmarked row drew a mark: %q", rows[0])
	}
	if !strings.Contains(rows[1], markedMark) {
		t.Errorf("the marked row lost its mark: %q", rows[1])
	}
	// And: the two measure the same, so marking shifts nothing.
	if lipgloss.Width(rows[0]) != lipgloss.Width(rows[1]) {
		t.Errorf("marking changed the row width: unmarked=%d marked=%d",
			lipgloss.Width(rows[0]), lipgloss.Width(rows[1]))
	}
}

// The bracket pair sits on the row the cursor is on, and on no other.
// Given three entries with the cursor on the middle one, when the rows render,
// then only that row carries the pair.
// Why it matters: the pair is what says which entry the space key will mark, so
// a pair on the wrong row marks a file the operator was not looking at.
func TestThePickerCursorCarriesTheBracketPair(t *testing.T) {
	// Given: three files, with the cursor on the middle one.
	dir := t.TempDir()
	for _, name := range []string{"a.txt", "b.txt", "c.txt"} {
		write(t, filepath.Join(dir, name))
	}
	entries := entriesIn(t, dir)

	// When: the rows render.
	rows := renderRows(t, 40, entries, map[string]bool{}, 1)

	// Then: the cursor row carries the pair and its neighbours do not.
	if !strings.Contains(rows[1], "[") || !strings.Contains(rows[1], "]") {
		t.Errorf("the cursor row lost its marker: %q", rows[1])
	}
	for _, i := range []int{0, 2} {
		if strings.Contains(rows[i], "[") || strings.Contains(rows[i], "]") {
			t.Errorf("row %d drew a marker it does not own: %q", i, rows[i])
		}
	}
}

// A picker row fits its column whatever the entry is named.
// Given names that carry wide characters, when a row renders at any width, then
// it measures exactly that width.
// Why it matters: a filename is chosen by whoever made the file, so the picker
// meets CJK and emoji names that measure more display cells than they hold
// runes, and a row wider than its column tears the frame around it.
func TestAPickerRowFitsItsColumn(t *testing.T) {
	// Given: a directory whose names mix wide and narrow characters.
	dir := t.TempDir()
	for _, name := range []string{
		"海辺の夕日.jpg",
		"文件名.txt",
		strings.Repeat("写", 60) + ".jpg",
		"holiday 🎉🎉🎉.jpg",
		"a-fairly-long-ascii-file-name.txt",
	} {
		write(t, filepath.Join(dir, name))
	}
	entries := entriesIn(t, dir)

	// When: every row renders at every width the panel can offer, marked and
	// not, so the state cell is swept as well as the label.
	for _, marked := range []map[string]bool{{}, allMarked(entries)} {
		for w := 8; w <= 120; w++ {
			for i, row := range renderRows(t, w, entries, marked, 0) {
				// Then: each occupies exactly that width.
				if cells := lipgloss.Width(row); cells != w {
					t.Fatalf("width %d: row %d (%s) measured %d: %q",
						w, i, entries[i].name, cells, row)
				}
			}
		}
	}
}

// An entry says what kind of thing it is.
// Given a directory, a symlink and a regular file, when each renders, then the
// directory and the symlink each carry their own suffix and the file carries
// none.
// Why it matters: marking a directory destroys everything under it and marking
// a symlink destroys only the link and never what it points at, so the three
// must not read alike in a list that feeds a run with no undo.
func TestAnEntrySaysWhatKindOfThingItIs(t *testing.T) {
	// Given: one of each, beside each other.
	dir := t.TempDir()
	write(t, filepath.Join(dir, "notes.txt"))
	if err := os.Mkdir(filepath.Join(dir, "projects"), 0o755); err != nil {
		t.Fatalf("make the directory: %v", err)
	}
	if err := os.Symlink(filepath.Join(dir, "notes.txt"), filepath.Join(dir, "shortcut")); err != nil {
		t.Fatalf("make the symlink: %v", err)
	}

	// When: the directory is read.
	byName := map[string]entry{}
	for _, e := range entriesIn(t, dir) {
		byName[e.name] = e
	}

	// Then: each label says which of the three it is.
	for name, want := range map[string]string{
		"notes.txt": "notes.txt",
		"projects":  "projects" + dirSuffix,
		"shortcut":  "shortcut" + linkSuffix,
	} {
		if got := byName[name].label; got != want {
			t.Errorf("%s rendered as %q, want %q", name, got, want)
		}
	}
}

func write(t *testing.T, path string) {
	t.Helper()
	if err := os.WriteFile(path, []byte("x"), 0o600); err != nil {
		t.Fatalf("write %s: %v", path, err)
	}
}

func allMarked(entries []entry) map[string]bool {
	marked := make(map[string]bool, len(entries))
	for _, e := range entries {
		marked[e.path] = true
	}
	return marked
}
