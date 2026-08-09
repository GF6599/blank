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
	"github.com/charmbracelet/bubbles/viewport"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"

	"blank/app"
)

const (
	padding  = 2
	maxWidth = 80

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
	args          []string
	jobs          int
	progressCh    chan float64
	activityCh    chan app.Event
	done          chan error
	engineCh      chan error
	engineErr     error
	engineSettled bool
	aborted       bool
	completed     bool
	ticking       bool
	rows          []activityRow
	contentWidth  int
	termHeight    int
}

var viewportWrapperStyle = lipgloss.NewStyle().MarginLeft(padding)

// UIStart boots the Bubble Tea UI and captures keyboard shortcuts for quitting
// or scrolling the activity viewport. It returns the engine's own error, and it
// does not return before the engine stops.
func UIStart(args []string, jobs int) error {
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
		args:       args,
		jobs:       jobs,
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
		startBlankCmd(m.ctx, m.args, m.jobs, m.progressCh, m.activityCh, m.done, m.engineCh),
		listenForProgressUpdate(m.ctx, m.progressCh),
		listenForActivityUpdate(m.ctx, m.activityCh),
		listenForEngine(m.engineCh),
	)
}

// Update reacts to user input and background events, keeping the UI state fresh.
func (m model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.KeyMsg:
		switch msg.String() {
		case "q", "esc", "ctrl+c":
			// Quitting stops the message loop, so the engine's outcome never
			// arrives. Bubble Tea renders the model returned here one last
			// time, which is the only chance to say the run was stopped.
			m.aborted = true
			m.cancel()
			return m, tea.Quit
		case "up", "k":
			m.viewport.ScrollUp(1)
			return m, nil
		case "down", "j":
			m.viewport.ScrollDown(1)
			return m, nil
		case "pgup", "b":
			m.viewport.HalfPageUp()
			return m, nil
		case "pgdown", "f":
			m.viewport.HalfPageDown()
			return m, nil
		default:
			return m, nil
		}

	case tea.WindowSizeMsg:
		m.termHeight = msg.Height
		m.contentWidth = max(min(maxWidth, msg.Width-padding*2), 8)
		m.viewport.Width = m.contentWidth - 2 - scrollbarCells
		m.progress.Width = m.contentWidth - reservedForControls(m.contentWidth)

		m.viewport.Height = max(min(activityRows, msg.Height-chromeRows-m.markLines()), 3)
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
		m.viewport.GotoBottom()
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

// The mark degrades to a single line on a small terminal, so the viewport height
// has to read this rather than assume markRows.
func (m model) markLines() int {
	if markVisible(m.contentWidth, m.termHeight) {
		return markRows
	}
	return 1
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
// moves down the list.
func (m model) activityView() string {
	rows := make([]string, len(m.rows))
	for i, r := range m.rows {
		rows[i] = markerRow(m.viewport.Width, m.stateCell(r.stage), r.text, i == len(m.rows)-1 && !m.completed)
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
	b.WriteString(pad)
	b.WriteString(statusStyleFor(outcome).Render(clampHead(statusLine(outcome, survivors), m.contentWidth)))
	b.WriteString("\n\n")

	b.WriteString(pad)
	b.WriteString(frameTop(m.contentWidth, panelTitle))
	b.WriteString("\n")
	b.WriteString(viewportWrapperStyle.Render(panelSides.Render(m.panelBody())))
	return b.String()
}

// The outcome goes to two receivers that cannot share one channel. UIStart
// receives after the program has returned, and a command started before the
// quit can outlive it, so whichever receiver won a shared channel would leave
// the other blocked on an empty one. Both of these are buffered, so neither
// send waits for a reader that may never come.
func startBlankCmd(ctx context.Context, args []string, jobs int, progress chan<- float64, activity chan<- app.Event, done, engine chan<- error) tea.Cmd {
	return func() tea.Msg {
		go func() {
			err := app.Blank(ctx, args, jobs, progress, activity)
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
