// Package tui renders the Bubble Tea-based status UI, showing progress and
// live activity logs while a run proceeds in the background.
package tui

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"slices"
	"strings"

	"github.com/charmbracelet/bubbles/progress"
	"github.com/charmbracelet/bubbles/spinner"
	"github.com/charmbracelet/bubbles/textinput"
	"github.com/charmbracelet/bubbles/viewport"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"

	"blank/app"
)

const (
	padding  = 2
	maxWidth = 80

	// Every wording the chrome can show has to survive this, because below it
	// there is nothing left to cut that still says which outcome a run reached.
	minContentWidth = 8

	panelTitle = "activity"

	// Everything the view draws besides the mark and the viewport: a leading
	// blank, a blank under the mark, the progress row, the status row, a blank,
	// the title rule, and the panel's bottom rule.
	chromeRows = 7

	// The panel holds this many rows whatever the terminal offers. Bubble Tea
	// runs inline, so a view sized to the screen would claim the whole scrollback
	// position and lose its top rows to the renderer's height clamp.
	activityRows = 8

	// Rows the panel keeps. Every message re-renders every retained row, so an
	// uncapped history costs the square of the file count and holds every path
	// in memory. This is many screens of scrollback, and `--log` writes the full
	// manifest for anyone who needs all of it.
	activityHistory = 200
)

type (
	gsCompleteMsg struct{}
	progressMsg   float64
	activityMsg   app.Event

	// engineDoneMsg carries the value the exit code is built from, so the
	// interface reports the run from the same verdict the caller receives.
	engineDoneMsg struct{ err error }
)

// activityRow is one target's line in the panel. path is what a settle event
// names it by, and text is what the row reads as.
type activityRow struct {
	path  string
	text  string
	stage app.Stage
}

type model struct {
	progress      progress.Model
	viewport      viewport.Model
	spinner       spinner.Model
	ctx           context.Context
	cancel        context.CancelFunc
	plan          app.Plan
	opts          app.Options
	progressCh    chan float64
	activityCh    chan app.Event
	done          chan error
	engineCh      chan error
	engineErr     error
	engineSettled bool
	aborted       bool
	completed     bool
	ticking       bool
	// Named for the false case, so a model built as a literal follows the run
	// without having to say so.
	detached bool
	rows     []activityRow
	// filterText narrows what the panel draws and never what the model holds,
	// so a target that settles behind the filter still settles.
	filterText   string
	filtering    bool
	filter       textinput.Model
	contentWidth int
	termHeight   int
}

// The filter is built when it opens rather than held in the model, because the
// only state worth keeping between two opens is the text, which already is.
func newLogFilter(text string) textinput.Model {
	ti := textinput.New()
	ti.Prompt = "filter: "
	ti.PromptStyle = hintStyle
	ti.TextStyle = rowStyle
	// The clipboard read execs pbpaste on macOS and xclip elsewhere, and every
	// other thing blank does is a standard library call.
	ti.KeyMap.Paste.Unbind()
	ti.SetValue(text)
	ti.CursorEnd()
	ti.Focus()
	return ti
}

var viewportWrapperStyle = lipgloss.NewStyle().MarginLeft(padding)

// UIStart boots the Bubble Tea UI and captures keyboard shortcuts for quitting
// or scrolling the activity viewport. It returns the engine's own error, and it
// does not return before the engine stops.
func UIStart(plan app.Plan, opts app.Options) error {
	ctx, cancel := context.WithCancel(context.Background())
	vp := viewport.New(0, 0)
	progressBar := progress.New(progress.WithScaledGradient(rosePineLove, rosePinePine))
	progressBar.EmptyColor = rosePineHighlightMed
	progressBar.PercentageStyle = lipgloss.NewStyle().Foreground(colorSubtle)
	sp := spinner.New(spinner.WithSpinner(spinner.MiniDot))
	sp.Style = busyStyle
	m := model{
		progress:   progressBar,
		viewport:   vp,
		spinner:    sp,
		ctx:        ctx,
		cancel:     cancel,
		plan:       plan,
		opts:       opts,
		progressCh: make(chan float64),
		activityCh: make(chan app.Event),
		done:       make(chan error, 1),
		engineCh:   make(chan error, 1),
		rows:       make([]activityRow, 0, 16),
	}
	defer cancel()

	_, runErr := tea.NewProgram(m).Run()
	// Stop the engine before waiting for it. Once the interface is gone nobody
	// reads the progress and activity channels, so a send would block forever.
	cancel()
	if runErr != nil {
		slog.Error("TUI error", "ERR", runErr)
		return fmt.Errorf("interface failed: %w", runErr)
	}
	return <-m.done
}

// Init satisfies Bubble Tea's Model interface by wiring all startup commands.
func (m model) Init() tea.Cmd {
	return tea.Batch(
		startBlankCmd(m.ctx, m.plan, m.opts, m.progressCh, m.activityCh, m.done, m.engineCh),
		listenForProgressUpdate(m.ctx, m.progressCh),
		listenForActivityUpdate(m.ctx, m.activityCh),
		listenForEngine(m.engineCh),
	)
}

// Update reacts to user input and background events, keeping the UI state fresh.
func (m model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.KeyMsg:
		// Ahead of the filter, because this screen is destroying files and the
		// key that stops it must never become text in an input.
		if msg.Type == tea.KeyCtrlC {
			m.aborted = true
			m.cancel()
			return m, tea.Quit
		}
		if m.filtering {
			return m.updateFilter(msg)
		}
		switch msg.String() {
		case "q", "esc":
			if msg.String() == "esc" && m.filterText != "" {
				// Escape backs out of the filter before it backs out of the
				// run, the way it does in the selector.
				m.filterText = ""
				m.viewport.SetContent(m.activityView())
				return m, nil
			}
			// Quitting stops the message loop, so the engine's outcome never
			// arrives. Bubble Tea renders the model returned here one last
			// time, which is the only chance to say the run was stopped.
			m.aborted = true
			m.cancel()
			return m, tea.Quit
		case "/":
			m.filter = newLogFilter(m.filterText)
			m.filtering = true
			return m, textinput.Blink
		case "up", "k":
			m.viewport.ScrollUp(1)
		case "down", "j":
			m.viewport.ScrollDown(1)
		case "pgup", "b":
			m.viewport.HalfPageUp()
		case "pgdown", "f":
			m.viewport.HalfPageDown()
		default:
			return m, nil
		}
		m.detached = !m.viewport.AtBottom()
		return m, nil

	case tea.WindowSizeMsg:
		m.termHeight = msg.Height
		m.contentWidth = max(min(maxWidth, msg.Width-padding*2), minContentWidth)
		m.viewport.Width = m.contentWidth - 2 - scrollbarCells
		m.progress.Width = m.contentWidth - reservedForControls(m.contentWidth)

		m.viewport.Height = max(min(activityRows, msg.Height-chromeRows-markLines(m.contentWidth, m.termHeight)), 3)
		m.viewport.SetContent(m.activityView())
		return m, nil

	case engineDoneMsg:
		m.engineErr = msg.err
		m.engineSettled = true
		return m, nil

	case gsCompleteMsg:
		if !m.completed && m.progress.Percent() < 1 {
			// Leave completed=false so UI shows default prompt when run aborted early.
			m.cancel()
			return m, nil
		}
		if !m.completed {
			m.completed = true
			m.progress.SetPercent(1)
		}
		m.cancel()
		return m, nil

	case progressMsg:
		if m.completed {
			// Drain any remaining updates until the channel closes.
			return m, listenForProgressUpdate(m.ctx, m.progressCh)
		}
		m.progress.SetPercent(float64(msg))
		if m.progress.Percent() >= 1 {
			m.completed = true
			m.cancel()
			return m, listenForProgressUpdate(m.ctx, m.progressCh)
		}
		// Keep listening for the next progress update
		return m, listenForProgressUpdate(m.ctx, m.progressCh)

	case activityMsg:
		m.note(app.Event(msg))
		m.viewport.SetContent(m.activityView())
		if !m.detached {
			m.viewport.GotoBottom()
		}
		cmds := []tea.Cmd{listenForActivityUpdate(m.ctx, m.activityCh)}
		// The animation is driven by one tick that re-arms itself, so it is
		// started only when nothing is already driving it.
		if !m.ticking && m.running() {
			m.ticking = true
			cmds = append(cmds, m.spinner.Tick)
		}
		return m, tea.Batch(cmds...)

	case spinner.TickMsg:
		if m.completed || !m.running() {
			m.ticking = false
			return m, nil
		}
		var cmd tea.Cmd
		m.spinner, cmd = m.spinner.Update(msg)
		m.viewport.SetContent(m.activityView())
		return m, cmd

	default:
		return m, nil
	}
}

// updateFilter owns every key while the input is open. The panel narrows as the
// text is typed, so what is on screen is always the answer to what is in the
// box. Enter puts the box away and keeps the text, escape throws both away.
func (m model) updateFilter(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	var cmd tea.Cmd
	switch msg.Type {
	case tea.KeyEnter:
		m.filtering = false
	case tea.KeyEsc:
		m.filtering, m.filterText = false, ""
	default:
		m.filter, cmd = m.filter.Update(msg)
		m.filterText = m.filter.Value()
	}
	m.viewport.SetContent(m.activityView())
	if !m.detached {
		m.viewport.GotoBottom()
	}
	return m, cmd
}

// matches is a fold-insensitive substring rather than a fuzzy match, because a
// log is searched for a fragment of a path and a fuzzy match answers with every
// row that happens to share those letters in that order.
func (m model) matches(r activityRow) bool {
	return m.filterText == "" ||
		strings.Contains(strings.ToLower(r.text), strings.ToLower(m.filterText))
}

func (m model) shownRows() int {
	if m.filterText == "" {
		return len(m.rows)
	}
	shown := 0
	for _, r := range m.rows {
		if m.matches(r) {
			shown++
		}
	}
	return shown
}

// A filtered panel looks exactly like a short log, so the heading carries the
// filter and both counts rather than leaving the difference to be discovered.
func (m model) panelHeading() string {
	if m.filterText == "" {
		return panelTitle
	}
	return fmt.Sprintf("%s  %s  %d of %d", panelTitle, m.filterText, m.shownRows(), len(m.rows))
}

// note appends a target the run has reached, or settles one it already holds.
// A settle is searched for from the newest row, because a target settles close
// behind its own start. One the history cap has already dropped is left alone,
// so a stale event cannot settle an unrelated file.
func (m *model) note(e app.Event) {
	if e.Stage == app.StageStart {
		m.rows = append(m.rows, activityRow{path: e.Path, text: e.Verb + " " + e.Path, stage: e.Stage})
		if len(m.rows) > activityHistory {
			m.rows = slices.Delete(m.rows, 0, len(m.rows)-activityHistory)
		}
		return
	}
	for i := len(m.rows) - 1; i >= 0; i-- {
		if m.rows[i].path == e.Path {
			m.rows[i].stage = e.Stage
			return
		}
	}
}

// outcome reads the engine's own error rather than counting the rows that carry
// a failed mark. The history is capped, and a target that fails during
// enumeration is never queued and so never reaches a row at all, so the panel
// knows a subset of what survived and the engine knows all of it.
func (m model) outcome() (runOutcome, int) {
	switch {
	case m.engineErr != nil:
		var incomplete *app.IncompleteRun
		if errors.As(m.engineErr, &incomplete) {
			return runFailed, len(incomplete.Failures)
		}
		return runFailed, 0
	case m.engineSettled && m.completed:
		return runComplete, 0
	case m.engineSettled:
		return runCancelled, 0
	// The operator stopped a run the engine has not reported on yet. Quitting
	// is also how a finished run is dismissed, so this is read after the
	// verdict rather than before it.
	case m.aborted:
		return runCancelled, 0
	default:
		return runActive, 0
	}
}

func (m model) running() bool {
	return slices.ContainsFunc(m.rows, func(r activityRow) bool { return r.stage == app.StageStart })
}

// A settled row shows nothing, so what is left on screen is what still needs
// reading: the work in flight, and every target that survived it.
func (m model) stateCell(stage app.Stage) string {
	switch stage {
	case app.StageFailed:
		return failedStyle.Render(failedMark)
	case app.StageDone:
		return " "
	default:
		return m.spinner.View()
	}
}

// Only the newest line is current, so the bracket pair tracks the work as it
// moves down the list. The pair belongs to the newest row of the whole log and
// not of what survived the filter, because it marks the target in flight rather
// than the last row drawn.
func (m model) activityView() string {
	rows := make([]string, 0, len(m.rows))
	newest := len(m.rows) - 1
	for i, r := range m.rows {
		if !m.matches(r) {
			continue
		}
		rows = append(rows, markerRow(m.viewport.Width, m.stateCell(r.stage), r.text, i == newest && !m.completed))
	}
	return strings.Join(rows, "\n")
}

// The viewport renders every row at its full width, blank rows included, so the
// cell appended to each lands in one column.
func (m model) panelBody() string {
	rows := strings.Split(m.viewport.View(), "\n")
	bar := scrollbar(len(rows), m.viewport.TotalLineCount(), m.viewport.YOffset)
	for i := range rows {
		rows[i] += bar[i]
	}
	return strings.Join(rows, "\n")
}

// Every block shares one content column, so they line up on both edges.
func (m model) View() string {
	pad := strings.Repeat(" ", padding)
	var b strings.Builder

	b.WriteString("\n")
	for _, line := range strings.Split(renderMark(m.contentWidth, m.termHeight), "\n") {
		b.WriteString(pad)
		b.WriteString(line)
		b.WriteString("\n")
	}
	b.WriteString("\n")

	bar := m.progress.ViewAs(m.progress.Percent())
	b.WriteString(pad)
	b.WriteString(bar)
	if ctrl := controlsFor(m.contentWidth); ctrl != "" {
		gap := max(m.contentWidth-lipgloss.Width(bar)-lipgloss.Width(ctrl), 1)
		b.WriteString(strings.Repeat(" ", gap))
		b.WriteString(renderControls(ctrl))
	}
	b.WriteString("\n")

	outcome, survivors := m.outcome()
	status := statusStyleFor(outcome).Render(statusLine(outcome, survivors, m.contentWidth))
	b.WriteString(pad)
	b.WriteString(status)
	if m.filtering {
		// The box is capped against the room the verdict leaves, so a long
		// filter scrolls inside itself rather than overrunning the column.
		input := m.filter
		input.Width = max(m.contentWidth-lipgloss.Width(status)-lipgloss.Width(input.Prompt)-2, 4)
		rendered := clampHead(input.View(), max(m.contentWidth-lipgloss.Width(status)-1, 0))
		gap := max(m.contentWidth-lipgloss.Width(status)-lipgloss.Width(rendered), 1)
		b.WriteString(strings.Repeat(" ", gap))
		b.WriteString(rendered)
	}
	b.WriteString("\n\n")

	b.WriteString(pad)
	b.WriteString(frameTop(m.contentWidth, m.panelHeading()))
	b.WriteString("\n")
	b.WriteString(viewportWrapperStyle.Render(panelSides.Render(m.panelBody())))
	return b.String()
}

// The outcome goes to two receivers that cannot share one channel. UIStart
// receives after the program has returned, and a command started before the
// quit can outlive it, so whichever receiver won a shared channel would leave
// the other blocked on an empty one. Both of these are buffered, so neither
// send waits for a reader that may never come.
func startBlankCmd(ctx context.Context, plan app.Plan, opts app.Options, progress chan<- float64, activity chan<- app.Event, done, engine chan<- error) tea.Cmd {
	return func() tea.Msg {
		go func() {
			err := app.Blank(ctx, plan, opts, progress, activity)
			engine <- err
			done <- err
		}()
		return nil
	}
}

// listenForEngine reads no context, because the engine writes exactly once to a
// buffered channel and the interface has to report the outcome of a run it has
// already cancelled.
func listenForEngine(engine <-chan error) tea.Cmd {
	return func() tea.Msg {
		return engineDoneMsg{err: <-engine}
	}
}

// listenForProgressUpdate forwards progress channel events to Bubble Tea.
func listenForProgressUpdate(ctx context.Context, progress <-chan float64) tea.Cmd {
	return func() tea.Msg {
		for {
			select {
			case <-ctx.Done():
				return gsCompleteMsg{}
			case p, ok := <-progress:
				if !ok { // channel closed -> completed
					return gsCompleteMsg{}
				}
				return progressMsg(p)
			}
		}
	}
}

// listenForActivityUpdate streams activity updates into the program loop.
func listenForActivityUpdate(ctx context.Context, activity <-chan app.Event) tea.Cmd {
	return func() tea.Msg {
		for {
			select {
			case <-ctx.Done():
				return gsCompleteMsg{}
			case a, ok := <-activity:
				if !ok {
					return gsCompleteMsg{}
				}
				return activityMsg(a)
			}
		}
	}
}
