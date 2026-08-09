package tui

import (
	"fmt"
	"io"
	"os"
	"path/filepath"

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
}

func newEntry(dir string, de os.DirEntry) entry {
	name := de.Name()
	return entry{
		path:  filepath.Join(dir, name),
		name:  name,
		label: name + suffixFor(de),
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

// The filter stays on, because it is the reason this is a list rather than the
// viewport the activity panel uses.
func newPickerList(marked map[string]bool) list.Model {
	l := list.New(nil, entryDelegate{marked: marked}, 0, 0)
	l.SetShowTitle(false)
	l.SetShowStatusBar(false)
	l.SetShowPagination(false)
	l.SetShowHelp(false)
	return l
}
