package tui

import (
	"fmt"
	"io"
	"log/slog"
	"os"
	"path/filepath"
	"slices"
	"strings"

	"github.com/charmbracelet/bubbles/list"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"

	"blank/app"
)

const (
	dirSuffix  = "/"
	linkSuffix = "@"

	// ASCII, for the reason failedMark is: it must be readable on any terminal
	// and in any font.
	markedMark  = "*"
	refusedMark = "-"
)

type entry struct {
	path  string
	name  string
	label string
	// isDir is false for a symlink to a directory, because os.ReadDir reports
	// the entry rather than what it points at, and the engine reads it the same
	// way.
	isDir bool
	// refusal is why the engine would decline this path, decided when the
	// listing is read so that a render never touches the filesystem.
	refusal error
}

func newEntry(dir string, de os.DirEntry, guard *app.Guard) entry {
	name := de.Name()
	path := filepath.Join(dir, name)
	return entry{
		path:    path,
		name:    name,
		label:   name + suffixFor(de),
		isDir:   de.IsDir(),
		refusal: guard.Refuses(path),
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
// A refused one is muted, because it is the one row in the listing that no key
// does anything to.
func (d entryDelegate) stateCell(e entry) string {
	switch {
	case e.refusal != nil:
		return hintStyle.Render(refusedMark)
	case d.marked[e.path]:
		return failedStyle.Render(markedMark)
	default:
		return " "
	}
}

const (
	// The picker owns the screen while it is up, so it shows more rows than the
	// activity panel does beside a progress bar.
	pickerRows = 16

	// A leading blank, a blank under the mark, the status row, a blank, the
	// title rule, and the listing's bottom rule.
	pickerChromeRows = 6
)

type pickerModel struct {
	list    list.Model
	dir     string
	marked  map[string]bool
	readErr error
	// refusal is the answer to the key just pressed, so it lives until the next
	// one rather than staying on screen over an unrelated row.
	refusal      error
	quitting     bool
	confirmed    bool
	contentWidth int
	termHeight   int
}

func newPickerModel(dir string) pickerModel {
	// filepath.Dir(".") is ".", so a picker holding the name it was given can
	// never reach the parent of the directory it started in.
	if abs, err := filepath.Abs(dir); err == nil {
		dir = abs
	}
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
	// One guard for the whole listing. Building it resolves every protected
	// path, which is a read of the filesystem per symlink on the list.
	guard := app.NewGuard()
	entries := make([]entry, 0, len(found))
	for _, de := range found {
		entries = append(entries, newEntry(dir, de, guard))
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
	if e.refusal != nil {
		// The guard refuses the whole command line, so a protected path taken
		// here would cancel every ordinary target beside it after the operator
		// had finished choosing them.
		m.refusal = e.refusal
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
		m.termHeight = msg.Height
		m.contentWidth = max(min(maxWidth, msg.Width-padding*2), minContentWidth)
		rows := max(min(pickerRows, msg.Height-pickerChromeRows-markLines(m.contentWidth, m.termHeight)), 3)
		m.list.SetSize(m.contentWidth-2, rows)
		return m, nil

	case tea.KeyMsg:
		m.refusal = nil
		// Ahead of the filter, because the list's own quit bindings are off and
		// a screen that routes ctrl+c into a text input cannot be left at all.
		if msg.Type == tea.KeyCtrlC {
			m.quitting = true
			return m, tea.Quit
		}
		// A filename holds spaces and every letter the picker binds, so while
		// the filter is being typed every other key is text rather than an
		// action.
		if m.list.SettingFilter() {
			break
		}
		switch msg.String() {
		case "q":
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

// The wordings the status row can offer, longest first, in the shape the run
// screen uses. Quitting is the last thing dropped, because it is how an operator
// leaves a screen they opened by mistake.
var pickerControlWordings = []string{
	"[space] mark  [enter] done  [q] quit",
	"[enter] done  [q] quit",
	"[q] quit",
}

func (m pickerModel) title() string {
	// frameTop spends two corners, the lead rune and a cell either side of the
	// label, and it needs one cell of fill left over.
	return clampTail(m.dir, max(m.contentWidth-6, 0))
}

// A directory nobody can read looks exactly like an empty one, so the reason
// takes the row rather than the count that would say nothing.
func (m pickerModel) statusText() string {
	switch {
	case m.refusal != nil:
		return failedStyle.Render(failedMark) + " " + m.refusal.Error()
	case m.list.SettingFilter():
		return m.list.FilterInput.View()
	case m.readErr != nil:
		return failedStyle.Render(failedMark) + " " + m.readErr.Error()
	default:
		shown, held := len(m.list.VisibleItems()), len(m.list.Items())
		position := 0
		if shown > 0 {
			position = m.list.Index() + 1
		}
		if shown != held {
			// An applied filter looks exactly like a small directory, so the
			// count it hid is named rather than left to be discovered.
			return fmt.Sprintf("%d/%d of %d  %d marked", position, shown, held, len(m.marked))
		}
		return fmt.Sprintf("%d/%d  %d marked", position, held, len(m.marked))
	}
}

func (m pickerModel) statusRow() string {
	left := m.statusText()
	var controls string
	for _, wording := range pickerControlWordings {
		if m.contentWidth-lipgloss.Width(left)-controlsGap >= lipgloss.Width(wording) {
			controls = wording
			break
		}
	}
	if controls == "" {
		// The tail, because the one status this row cannot fit is a read
		// failure and the reason it failed is the end of that sentence.
		return statusStyle.Render(clampTail(left, m.contentWidth))
	}
	gap := max(m.contentWidth-lipgloss.Width(left)-lipgloss.Width(controls), 1)
	return statusStyle.Render(left) + strings.Repeat(" ", gap) + renderControls(controls)
}

// Every block shares one content column, the way the run screen builds its own.
func (m pickerModel) View() string {
	pad := strings.Repeat(" ", padding)
	var b strings.Builder

	b.WriteString("\n")
	for _, line := range strings.Split(renderMark(m.contentWidth, m.termHeight), "\n") {
		b.WriteString(pad)
		b.WriteString(line)
		b.WriteString("\n")
	}
	b.WriteString("\n")

	b.WriteString(pad)
	b.WriteString(m.statusRow())
	b.WriteString("\n\n")

	b.WriteString(pad)
	b.WriteString(frameTop(m.contentWidth, m.title()))
	b.WriteString("\n")
	b.WriteString(viewportWrapperStyle.Render(panelSides.Render(m.listBody())))
	return b.String()
}

// The listing pads its own rows and its empty state does not, so a directory
// holding nothing would draw a frame the width of the words inside it.
func (m pickerModel) listBody() string {
	inner := m.contentWidth - 2
	rows := strings.Split(m.list.View(), "\n")
	for i, row := range rows {
		rows[i] = padCells(clampHead(row, inner), inner)
	}
	return strings.Join(rows, "\n")
}

// PickTargets runs the selector and returns the paths the operator marked, in a
// stable order. It returns nothing when the operator quit rather than confirmed,
// which is the same answer as marking nothing at all.
func PickTargets(start string) ([]string, error) {
	final, err := tea.NewProgram(newPickerModel(start)).Run()
	if err != nil {
		slog.Error("Selector error", "ERR", err)
		return nil, fmt.Errorf("selector failed: %w", err)
	}
	return final.(pickerModel).targets(), nil
}

// The filter stays on, because it is the reason this is a list rather than the
// viewport the activity panel uses.
func newPickerList(marked map[string]bool) list.Model {
	l := list.New(nil, entryDelegate{marked: marked}, 0, 0)
	l.SetShowTitle(false)
	l.SetShowStatusBar(false)
	// The status row above the frame draws the filter input, so leaving this on
	// spends a row of the listing on a second copy of it.
	l.SetShowFilter(false)
	l.SetShowPagination(false)
	l.SetShowHelp(false)
	l.SetStatusBarItemName("entry", "entries")
	l.DisableQuitKeybindings()
	// textinput binds ctrl+v to a clipboard read, which execs pbpaste on macOS
	// and xclip elsewhere. Every other thing blank does is a standard library
	// call, and a filename filter is no reason to start spawning processes.
	l.FilterInput.KeyMap.Paste.Unbind()
	return l
}
