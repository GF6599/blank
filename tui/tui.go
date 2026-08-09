// Package tui renders the Bubble Tea-based status UI, showing progress and
// live activity logs while a run proceeds in the background.
package tui

import (
	"context"
	"fmt"
	"log/slog"
	"strings"

	"github.com/charmbracelet/bubbles/progress"
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
)

type (
	gsCompleteMsg struct{}
	progressMsg   float64
	activityMsg   string
)

type model struct {
	progress     progress.Model
	viewport     viewport.Model
	ctx          context.Context
	cancel       context.CancelFunc
	args         []string
	progressCh   chan float64
	activityCh   chan string
	done         chan error
	completed    bool
	logs         []string
	contentWidth int
	termHeight   int
}

var viewportWrapperStyle = lipgloss.NewStyle().MarginLeft(padding)

// UIStart boots the Bubble Tea UI and captures keyboard shortcuts for quitting
// or scrolling the activity viewport. It returns the engine's own error, and it
// does not return before the engine stops.
func UIStart(args []string) error {
	ctx, cancel := context.WithCancel(context.Background())
	vp := viewport.New(0, 0)
	progressBar := progress.New(progress.WithScaledGradient(rosePineLove, rosePinePine))
	progressBar.EmptyColor = rosePineHighlightMed
	progressBar.PercentageStyle = lipgloss.NewStyle().Foreground(colorSubtle)
	m := model{
		progress:   progressBar,
		viewport:   vp,
		ctx:        ctx,
		cancel:     cancel,
		args:       args,
		progressCh: make(chan float64),
		activityCh: make(chan string),
		done:       make(chan error, 1),
		logs:       make([]string, 0, 16),
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
		startBlankCmd(m.ctx, m.args, m.progressCh, m.activityCh, m.done),
		listenForProgressUpdate(m.ctx, m.progressCh),
		listenForActivityUpdate(m.ctx, m.activityCh),
	)
}

// Update reacts to user input and background events, keeping the UI state fresh.
func (m model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.KeyMsg:
		switch msg.String() {
		case "q", "esc", "ctrl+c":
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
		m.viewport.Width = m.contentWidth - 2
		m.progress.Width = m.contentWidth - reservedForControls(m.contentWidth)

		m.viewport.Height = max(min(activityRows, msg.Height-chromeRows-m.markLines()), 3)
		m.viewport.SetContent(m.activityView())
		return m, nil

	// FrameMsg is sent when the progress bar wants to animate itself
	case progress.FrameMsg:
		progressModel, cmd := m.progress.Update(msg)
		m.progress = progressModel.(progress.Model)
		return m, cmd

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
		m.logs = append(m.logs, string(msg))
		m.viewport.SetContent(m.activityView())
		m.viewport.GotoBottom()
		return m, listenForActivityUpdate(m.ctx, m.activityCh)

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

// Only the newest line is current, so the bracket pair tracks the work as it
// moves down the list.
func (m model) activityView() string {
	rows := make([]string, len(m.logs))
	for i, line := range m.logs {
		rows[i] = markerRow(m.viewport.Width, line, i == len(m.logs)-1 && !m.completed)
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

	b.WriteString(pad)
	b.WriteString(statusStyle.Render(clampHead(statusLine(m.completed), m.contentWidth)))
	b.WriteString("\n\n")

	b.WriteString(pad)
	b.WriteString(frameTop(m.contentWidth, panelTitle))
	b.WriteString("\n")
	b.WriteString(viewportWrapperStyle.Render(panelSides.Render(m.viewport.View())))
	return b.String()
}

func startBlankCmd(ctx context.Context, args []string, progress chan<- float64, activity chan<- string, done chan<- error) tea.Cmd {
	return func() tea.Msg {
		go func() { done <- app.Blank(ctx, args, progress, activity) }()
		return nil
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
func listenForActivityUpdate(ctx context.Context, activity <-chan string) tea.Cmd {
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
