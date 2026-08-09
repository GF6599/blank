package tui

import (
	"fmt"
	"io"
	"os"
	"path/filepath"
	"slices"

	"github.com/charmbracelet/bubbles/list"
	tea "github.com/charmbracelet/bubbletea"
)

const (
	dirSuffix  = "/"
	linkSuffix = "@"

	// ASCII, for the reason failedMark is: it must be readable on any terminal
	// and in any font.
	markedMark = "*"
)

type entry struct {
	path  string
	name  string
	label string
	// isDir is false for a symlink to a directory, because os.ReadDir reports
	// the entry rather than what it points at, and the engine reads it the same
	// way.
	isDir bool
}

func newEntry(dir string, de os.DirEntry) entry {
	name := de.Name()
	return entry{
		path:  filepath.Join(dir, name),
		name:  name,
		label: name + suffixFor(de),
		isDir: de.IsDir(),
	}
}

// Marking a directory destroys the whole tree under it, and marking a symlink
// destroys the link and never what it points at. Neither is what the bare name
// suggests, so both say which they are.
func suffixFor(de os.DirEntry) string {
	switch {
	case de.Type()&os.ModeSymlink != 0:
		return linkSuffix
	case de.IsDir():
		return dirSuffix
	default:
		return ""
	}
}

func (e entry) FilterValue() string { return e.name }

func toItems(entries []entry) []list.Item {
	items := make([]list.Item, len(entries))
	for i, e := range entries {
		items[i] = e
	}
	return items
}

// entryDelegate reads the same map the picker marks into, so a mark drawn on a
// row and a mark the confirmation hands over cannot disagree.
type entryDelegate struct {
	marked map[string]bool
}

func (d entryDelegate) Height() int                         { return 1 }
func (d entryDelegate) Spacing() int                        { return 0 }
func (d entryDelegate) Update(tea.Msg, *list.Model) tea.Cmd { return nil }

func (d entryDelegate) Render(w io.Writer, m list.Model, index int, item list.Item) {
	e := item.(entry)
	fmt.Fprint(w, markerRow(m.Width(), d.stateCell(e), e.label, index == m.Index()))
}

// A marked entry is one the run will destroy, so it carries the colour the
// survivor mark already uses rather than the accent, which says something else.
func (d entryDelegate) stateCell(e entry) string {
	if d.marked[e.path] {
		return failedStyle.Render(markedMark)
	}
	return " "
}

type pickerModel struct {
	list      list.Model
	dir       string
	marked    map[string]bool
	readErr   error
	quitting  bool
	confirmed bool
}

func newPickerModel(dir string) pickerModel {
	marked := map[string]bool{}
	m := pickerModel{list: newPickerList(marked), dir: dir, marked: marked}
	m.load(dir)
	return m
}

// A directory that cannot be read keeps whatever os.ReadDir managed before it
// failed, beside the error, rather than reading as an empty directory.
func (m *pickerModel) load(dir string) {
	found, err := os.ReadDir(dir)
	m.dir, m.readErr = dir, err
	entries := make([]entry, 0, len(found))
	for _, de := range found {
		entries = append(entries, newEntry(dir, de))
	}
	// A cursor left past the end of a shorter listing selects nothing at all,
	// and a filter typed for one directory means nothing in the next.
	m.list.ResetFilter()
	m.list.SetItems(toItems(entries))
	m.list.Select(0)
}

func (m *pickerModel) toggle() {
	e, ok := m.selected()
	if !ok {
		return
	}
	if m.marked[e.path] {
		delete(m.marked, e.path)
		return
	}
	m.marked[e.path] = true
}

func (m pickerModel) selected() (entry, bool) {
	e, ok := m.list.SelectedItem().(entry)
	return e, ok
}

// A quit leaves its marks in the shared map, so the confirmation decides what
// the run receives and the map never does.
func (m pickerModel) targets() []string {
	if !m.confirmed {
		return nil
	}
	out := make([]string, 0, len(m.marked))
	for path := range m.marked {
		out = append(out, path)
	}
	// Map order varies between runs, and one set of marks has to produce one
	// plan for the operator to confirm.
	slices.Sort(out)
	return out
}

func (m pickerModel) Init() tea.Cmd { return nil }

func (m pickerModel) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.list.SetSize(msg.Width, msg.Height)
		return m, nil

	case tea.KeyMsg:
		// A filename holds spaces and every letter the picker binds, so while
		// the filter is being typed every key is text rather than an action.
		if m.list.SettingFilter() {
			break
		}
		switch msg.String() {
		case "ctrl+c", "q":
			m.quitting = true
			return m, tea.Quit
		case "esc":
			// Escape belongs to the filter first, and to the picker only when
			// there is no filter left to back out of.
			if m.list.FilterState() != list.Unfiltered {
				break
			}
			m.quitting = true
			return m, tea.Quit
		case "enter":
			m.quitting, m.confirmed = true, true
			return m, tea.Quit
		case " ":
			m.toggle()
			return m, nil
		case "l", "right":
			if e, ok := m.selected(); ok && e.isDir {
				m.load(e.path)
			}
			return m, nil
		case "h", "left", "backspace":
			if parent := filepath.Dir(m.dir); parent != m.dir {
				m.load(parent)
			}
			return m, nil
		}
	}
	var cmd tea.Cmd
	m.list, cmd = m.list.Update(msg)
	return m, cmd
}

func (m pickerModel) View() string { return m.list.View() }

// The filter stays on, because it is the reason this is a list rather than the
// viewport the activity panel uses.
func newPickerList(marked map[string]bool) list.Model {
	l := list.New(nil, entryDelegate{marked: marked}, 0, 0)
	l.SetShowTitle(false)
	l.SetShowStatusBar(false)
	l.SetShowPagination(false)
	l.SetShowHelp(false)
	l.DisableQuitKeybindings()
	// textinput binds ctrl+v to a clipboard read, which execs pbpaste on macOS
	// and xclip elsewhere. Every other thing blank does is a standard library
	// call, and a filename filter is no reason to start spawning processes.
	l.FilterInput.KeyMap.Paste.Unbind()
	return l
}
