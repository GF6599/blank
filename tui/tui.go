// Package tui
// Contains the code for showing a TUI progress bar
// using bubbletea as the framework
package tui

import (
	"context"
	"log/slog"
	"os"
	"strings"

	"github.com/charmbracelet/bubbles/progress"
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
)

type model struct {
	progress   progress.Model
	ctx        context.Context
	args       []string
	statusChan chan float64
}

var helpStyle = lipgloss.NewStyle().Foreground(lipgloss.Color("#626262")).Render

func UIStart(args []string) {
	// Redirect logs to a file so they don't interfere with the TUI
	if lf, err := os.OpenFile("gshred.log", os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o644); err == nil {
		logger := slog.New(slog.NewTextHandler(lf, &slog.HandlerOptions{}))
		slog.SetDefault(logger)
		defer lf.Close()
	}
	m := model{
		progress:   progress.New(progress.WithScaledGradient("#FF7CCB", "#FDFF8C")),
		ctx:        context.Background(),
		args:       args,
		statusChan: make(chan float64),
	}
	if _, err := tea.NewProgram(m, tea.WithAltScreen()).Run(); err != nil {
		slog.Error("TUI error", "ERR", err)
	}
}

func (m model) Init() tea.Cmd {
	return tea.Batch(startGsCmd(m.ctx, m.args, m.statusChan), listenForProgressUpdate(m.ctx, m.statusChan))
}

func (m model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.KeyMsg:
		return m, tea.Quit

	case tea.WindowSizeMsg:
		m.progress.Width = msg.Width - padding*2 - 4
		m.progress.Width = min(maxWidth, m.progress.Width)
		return m, nil

	// FrameMsg is sent when the progress bar wants to animate itself
	case progress.FrameMsg:
		progressModel, cmd := m.progress.Update(msg)
		m.progress = progressModel.(progress.Model)
		return m, cmd

	case gsCompleteMsg:
		return m, tea.Quit

	case progressMsg:
		if m.progress.Percent() >= 1 {
			return m, tea.Quit
		}
		m.progress.SetPercent(float64(msg))
		// Keep listening for the next progress update
		return m, listenForProgressUpdate(m.ctx, m.statusChan)

	default:
		return m, nil
	}
}

func (m model) View() string {
	pad := strings.Repeat(" ", padding)
	return "\n" +
		pad + m.progress.ViewAs(m.progress.Percent()) +
		pad + helpStyle("Press any key to quit")
}

func startGsCmd(ctx context.Context, args []string, progress chan<- float64) tea.Cmd {
	return func() tea.Msg {
		go gs.Gshred(ctx, args, progress)
		return nil
	}
}

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
