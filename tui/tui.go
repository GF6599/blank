// Package tui renders the Bubble Tea-based status UI, showing progress and
// live activity logs while shredding runs in the background.
package tui

import (
	"context"
	"log/slog"
	"strings"

	"github.com/charmbracelet/bubbles/progress"
	"github.com/charmbracelet/bubbles/viewport"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"

	gs "gshred/app"
)

const (
	padding  = 2
	maxWidth = 80
)

type (
	gsCompleteMsg struct{}
	progressMsg   float64
	activityMsg   string
)

// model captures Bubble Tea state for the shredding session, including the
// progress bar, scrollable viewport, and communication channels.
type model struct {
	progress   progress.Model
	viewport   viewport.Model
	ctx        context.Context
	cancel     context.CancelFunc
	args       []string
	progressCh chan float64
	activityCh chan string
	completed  bool
	logs       []string
}

var (
	helpStyle            = lipgloss.NewStyle().Foreground(lipgloss.Color("#626262")).Render
	logBoxStyle          = lipgloss.NewStyle().Border(lipgloss.NormalBorder()).Padding(0, 1)
	viewportWrapperStyle = lipgloss.NewStyle().MarginLeft(padding)
)

// UIStart boots the Bubble Tea UI and captures keyboard shortcuts for quitting
// or scrolling the activity viewport.
func UIStart(args []string) {
	ctx, cancel := context.WithCancel(context.Background())
	vp := viewport.New(0, 0)
	vp.Style = logBoxStyle
	m := model{
		progress:   progress.New(progress.WithScaledGradient("#eb6f92", "#31748f")),
		viewport:   vp,
		ctx:        ctx,
		cancel:     cancel,
		args:       args,
		progressCh: make(chan float64),
		activityCh: make(chan string),
		logs:       make([]string, 0, 16),
	}
	defer cancel()
	if _, err := tea.NewProgram(m, tea.WithAltScreen()).Run(); err != nil {
		slog.Error("TUI error", "ERR", err)
	}
}

// Init satisfies Bubble Tea's Model interface by wiring all startup commands.
func (m model) Init() tea.Cmd {
	return tea.Batch(
		startGsCmd(m.ctx, m.args, m.progressCh, m.activityCh),
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
		width := msg.Width - padding*2 - 4
		width = min(maxWidth, width)
		width = max(width, 10)
		m.progress.Width = width
		m.viewport.Width = width
		height := msg.Height - 8
		height = max(height, 5)
		m.viewport.Height = height
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
		m.viewport.SetContent(strings.Join(m.logs, "\n"))
		m.viewport.GotoBottom()
		return m, listenForActivityUpdate(m.ctx, m.activityCh)

	default:
		return m, nil
	}
}

// View lays out the progress bar, helper text, and log viewport.
func (m model) View() string {
	pad := strings.Repeat(" ", padding)
	status := "Press q or Esc to quit"
	if m.completed {
		status = "Shredding complete — press q or Esc to close"
	}
	var builder strings.Builder
	builder.WriteString("\n")
	builder.WriteString(pad)
	builder.WriteString(m.progress.ViewAs(m.progress.Percent()))
	builder.WriteString("\n")
	builder.WriteString(pad)
	builder.WriteString(helpStyle(status))
	builder.WriteString("\n\n")
	builder.WriteString(viewportWrapperStyle.Render(m.viewport.View()))
	return builder.String()
}

// startGsCmd launches the shredding worker asynchronously.
func startGsCmd(ctx context.Context, args []string, progress chan<- float64, activity chan<- string) tea.Cmd {
	return func() tea.Msg {
		go gs.Gshred(ctx, args, progress, activity)
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
