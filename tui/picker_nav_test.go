package tui

import (
	"os"
	"path/filepath"
	"slices"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
)

func press(t *testing.T, m pickerModel, keys ...string) pickerModel {
	t.Helper()
	for _, k := range keys {
		var msg tea.KeyMsg
		switch k {
		case " ":
			msg = tea.KeyMsg{Type: tea.KeySpace, Runes: []rune{' '}}
		case "enter":
			msg = tea.KeyMsg{Type: tea.KeyEnter}
		case "esc":
			msg = tea.KeyMsg{Type: tea.KeyEsc}
		case "backspace":
			msg = tea.KeyMsg{Type: tea.KeyBackspace}
		case "ctrl+v":
			msg = tea.KeyMsg{Type: tea.KeyCtrlV}
		case "ctrl+c":
			msg = tea.KeyMsg{Type: tea.KeyCtrlC}
		default:
			msg = tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune(k)}
		}
		var cmd tea.Cmd
		updated, cmd := m.Update(msg)
		m = updated.(pickerModel)
		if m.loading && cmd != nil {
			updated, _ = m.Update(cmd())
			m = updated.(pickerModel)
		}
	}
	return m
}

// The size holds every entry on one page, so a cursor index is a row index.
func openPicker(t *testing.T, dir string) pickerModel {
	t.Helper()
	m := newPickerModel(dir)
	updated, _ := m.Update(m.Init()())
	m = updated.(pickerModel)
	updated, _ = m.Update(tea.WindowSizeMsg{Width: 60, Height: 30})
	return updated.(pickerModel)
}

// The picker is ready to draw before it has read the directory it opened on.
// Given a directory holding one entry, when the picker is built, then it lists
// nothing until the load command has run and its message has arrived.
// Why it matters: a read inside Update blocks every key and every repaint, so a
// slow or remote directory would freeze the screen with no way to leave it.
func TestPickerLoadsDirectoriesThroughACommand(t *testing.T) {
	dir := t.TempDir()
	write(t, filepath.Join(dir, "notes.txt"))
	m := newPickerModel(dir)
	if len(m.list.Items()) != 0 || !m.loading {
		t.Fatal("the constructor read the directory synchronously")
	}
	cmd := m.Init()
	if cmd == nil {
		t.Fatal("Init returned no directory-loading command")
	}
	updated, _ := m.Update(cmd())
	m = updated.(pickerModel)
	if m.loading || len(m.list.Items()) != 1 {
		t.Fatalf("the loaded message left loading=%v with %d items", m.loading, len(m.list.Items()))
	}
}

func cursorOn(t *testing.T, m pickerModel, name string) pickerModel {
	t.Helper()
	for i, item := range m.list.VisibleItems() {
		if item.(entry).name == name {
			m.list.Select(i)
			return m
		}
	}
	t.Fatalf("%q is not in the listing of %s", name, m.dir)
	return m
}

// Marking the same entry twice leaves it unmarked.
// Given the cursor on a file, when space is pressed twice, then the picker hands
// over nothing.
// Why it matters: space is the only way to undo a mark, so a toggle that only
// adds makes a mistaken mark unrecoverable without quitting the whole picker.
func TestMarkingAnEntryTwiceLeavesItUnmarked(t *testing.T) {
	// Given: a directory holding one file, with the cursor on it.
	dir := t.TempDir()
	write(t, filepath.Join(dir, "notes.txt"))
	m := openPicker(t, dir)

	// When: it is marked and then marked again, and the set is confirmed.
	m = press(t, m, " ", " ", "enter")

	// Then: nothing is handed over.
	if got := m.targets(); len(got) != 0 {
		t.Fatalf("the second mark did not undo the first: %v", got)
	}
}

// A mark holds while the operator browses elsewhere.
// Given a file marked inside a subdirectory, when the picker goes up and back
// down and confirms, then that file is still in the set.
// Why it matters: targets in more than one directory are the reason the picker
// marks a set rather than taking one path, so a mark lost on navigation makes
// the whole feature single-select by accident.
func TestAMarkSurvivesLeavingTheDirectoryAndComingBack(t *testing.T) {
	// Given: a file inside a subdirectory, marked.
	root := t.TempDir()
	child := filepath.Join(root, "child")
	if err := os.Mkdir(child, 0o755); err != nil {
		t.Fatalf("make the subdirectory: %v", err)
	}
	target := filepath.Join(child, "secret.txt")
	write(t, target)
	write(t, filepath.Join(root, "other.txt"))

	m := openPicker(t, root)
	m = cursorOn(t, m, "child")
	m = press(t, m, "l")
	m = cursorOn(t, m, "secret.txt")
	m = press(t, m, " ")

	// When: the picker goes up, back down, and the set is confirmed.
	m = press(t, m, "h")
	m = cursorOn(t, m, "child")
	m = press(t, m, "l", "enter")

	// Then: the mark is still in the set that goes to the run.
	if got := m.targets(); !slices.Equal(got, []string{target}) {
		t.Fatalf("marked set is %v, want %v", got, []string{target})
	}
}

// Moving to another directory leaves the old cursor and filter behind.
// Given a filter typed in a large directory, when the picker descends into a
// smaller one, then the cursor sits on its first entry and the filter is gone.
// Why it matters: a cursor left past the end of a shorter listing selects
// nothing, so space and the descend key both stop working with no sign why.
func TestMovingDirectoryResetsTheCursorAndTheFilter(t *testing.T) {
	// Given: a directory of many files holding one directory of a single file.
	root := t.TempDir()
	for _, name := range []string{"a.txt", "b.txt", "c.txt", "d.txt", "e.txt"} {
		write(t, filepath.Join(root, name))
	}
	child := filepath.Join(root, "zz-child")
	if err := os.Mkdir(child, 0o755); err != nil {
		t.Fatalf("make the subdirectory: %v", err)
	}
	write(t, filepath.Join(child, "only.txt"))

	m := openPicker(t, root)
	m.list.SetFilterText("ch")
	m = cursorOn(t, m, "zz-child")

	// When: the picker descends.
	m = press(t, m, "l")

	// Then: the cursor is on the one entry there, so space marks it.
	if got := m.list.Index(); got != 0 {
		t.Errorf("the cursor stayed at index %d after the move", got)
	}
	// And: the filter did not follow.
	if got := m.list.FilterValue(); got != "" {
		t.Errorf("the filter %q followed the picker into %s", got, child)
	}
	m = press(t, m, " ", "enter")
	if got := m.targets(); !slices.Equal(got, []string{filepath.Join(child, "only.txt")}) {
		t.Fatalf("marking after the move gave %v", got)
	}
}

// While the filter is being typed, every key belongs to it.
// Given the filter open, when keys that would otherwise mark, navigate or quit
// are pressed, then they become filter text and the picker does not act on them.
// Why it matters: space, q, h and l are all ordinary characters in a filename,
// so a picker that acts on them while the operator is typing marks or destroys
// something nobody chose.
func TestTheFilterOwnsItsOwnKeys(t *testing.T) {
	// Given: a directory holding a file whose name is made of the action keys.
	dir := t.TempDir()
	write(t, filepath.Join(dir, "hql notes.txt"))
	child := filepath.Join(dir, "sub")
	if err := os.Mkdir(child, 0o755); err != nil {
		t.Fatalf("make the subdirectory: %v", err)
	}
	m := openPicker(t, dir)

	// When: the filter is opened and those keys are typed into it.
	m = press(t, m, "/", "h", "q", "l", " ")

	// Then: they reached the filter rather than the picker.
	if got := m.list.FilterValue(); got != "hql " {
		t.Fatalf("the filter holds %q, want %q", got, "hql ")
	}
	// And: the picker neither moved nor marked.
	if m.dir != dir {
		t.Errorf("the picker moved to %s while the filter was open", m.dir)
	}
	if len(m.marked) != 0 {
		t.Errorf("the filter marked %d entries", len(m.marked))
	}
}

// Escape backs out of the filter before it backs out of the picker.
// Given a filter that has been applied, when escape is pressed twice, then the
// first clears the filter and the second quits.
// Why it matters: escape is the only key bound to two jobs, and quitting on the
// first press throws away a marked set the operator was still building.
func TestEscapeClearsTheFilterBeforeItQuits(t *testing.T) {
	// Given: a directory with a filter applied over it.
	dir := t.TempDir()
	write(t, filepath.Join(dir, "notes.txt"))
	m := openPicker(t, dir)
	m = press(t, m, "/", "n", "enter")

	// When: escape is pressed once.
	m = press(t, m, "esc")

	// Then: the filter is gone and the picker is still open.
	if got := m.list.FilterValue(); got != "" {
		t.Fatalf("escape left the filter %q in place", got)
	}
	if m.quitting {
		t.Fatal("escape quit the picker while a filter was applied")
	}

	// When: escape is pressed again, then the picker gives up.
	if m = press(t, m, "esc"); !m.quitting {
		t.Fatal("escape did not quit an unfiltered picker")
	}
}

// Ctrl+C leaves the picker even while the filter is open.
// Given a filter being typed, when ctrl+c is pressed, then the picker quits and
// hands over nothing.
// Why it matters: ctrl+c is the key an operator reaches for when a program is
// not doing what they expect, and the list's own quit bindings are turned off,
// so a screen that routes it into a text input cannot be left at all.
func TestCtrlCLeavesThePickerWhileTheFilterIsOpen(t *testing.T) {
	// Given: a directory with a filter half typed over it.
	dir := t.TempDir()
	write(t, filepath.Join(dir, "notes.txt"))
	m := openPicker(t, dir)
	m = press(t, m, " ", "/", "n", "o")
	if !m.list.SettingFilter() {
		t.Fatal("the setup did not open the filter")
	}

	// When: ctrl+c is pressed.
	m = press(t, m, "ctrl+c")

	// Then: the picker gives up, and hands over nothing.
	if !m.quitting {
		t.Fatal("ctrl+c was swallowed by the filter input")
	}
	if got := m.targets(); len(got) != 0 {
		t.Fatalf("ctrl+c handed over %v", got)
	}
}

// Quitting hands over nothing, whatever was marked first.
// Given two files marked, when the picker is quit rather than confirmed, then it
// hands over no targets at all.
// Why it matters: quitting is how an operator backs out of a screen they opened
// by mistake, and this one feeds a run that cannot be undone.
func TestQuittingHandsOverNothing(t *testing.T) {
	// Given: a directory with both its files marked.
	dir := t.TempDir()
	write(t, filepath.Join(dir, "a.txt"))
	write(t, filepath.Join(dir, "b.txt"))
	m := openPicker(t, dir)
	m = press(t, m, " ", "j", " ")
	if len(m.marked) != 2 {
		t.Fatalf("the setup marked %d entries, want 2", len(m.marked))
	}

	// When: the picker is quit.
	m = press(t, m, "q")

	// Then: nothing goes to the run.
	if got := m.targets(); len(got) != 0 {
		t.Fatalf("quitting handed over %v", got)
	}
}

// The picker does not walk into a symlink.
// Given a symlink pointing at a directory, when the descend key is pressed on
// it, then the picker stays where it is.
// Why it matters: the engine unlinks a symlink and never reads through one, so a
// picker that descends offers targets the operator would read as being inside
// the directory they are browsing when they are somewhere else entirely.
func TestThePickerDoesNotWalkIntoASymlink(t *testing.T) {
	// Given: a symlink beside the directory it points at.
	root := t.TempDir()
	real := filepath.Join(root, "real")
	if err := os.Mkdir(real, 0o755); err != nil {
		t.Fatalf("make the directory: %v", err)
	}
	write(t, filepath.Join(real, "inside.txt"))
	if err := os.Symlink(real, filepath.Join(root, "shortcut")); err != nil {
		t.Fatalf("make the symlink: %v", err)
	}

	m := openPicker(t, root)
	m = cursorOn(t, m, "shortcut")

	// When: the descend key is pressed on the symlink.
	m = press(t, m, "l")

	// Then: the picker is still in the directory it started in.
	if m.dir != root {
		t.Fatalf("the picker followed the symlink to %s", m.dir)
	}
	// And: marking it names the link, not what it points at.
	m = press(t, m, " ", "enter")
	if got := m.targets(); !slices.Equal(got, []string{filepath.Join(root, "shortcut")}) {
		t.Fatalf("marking the symlink gave %v", got)
	}
}
