package tui

import (
	"context"
	"fmt"
	"strings"
	"testing"

	"github.com/charmbracelet/bubbles/progress"
	"github.com/charmbracelet/bubbles/viewport"
	tea "github.com/charmbracelet/bubbletea"
)

func previewModel(t *testing.T, w, h int, pct float64, logs []string, done bool) model {
	t.Helper()
	ctx, cancel := context.WithCancel(context.Background())
	t.Cleanup(cancel)
	vp := viewport.New(0, 0)
	bar := progress.New(progress.WithScaledGradient(rosePineLove, rosePinePine))
	bar.EmptyColor = rosePineHighlightMed
	m := model{
		progress: bar,
		viewport: vp,
		ctx:      ctx,
		cancel:   cancel,
		logs:     logs,
	}
	m.completed = done
	updated, _ := m.Update(tea.WindowSizeMsg{Width: w, Height: h})
	m = updated.(model)
	m.progress.SetPercent(pct)
	m.viewport.SetContent(m.activityView())
	m.viewport.GotoBottom()
	return m
}

func TestPreviewRender(t *testing.T) {
	logs := []string{
		"Overwriting /Users/dev/Documents/secret-report.pdf",
		"Overwriting /Users/dev/Documents/keys/id_rsa",
		"Deleting directory /Users/dev/Documents/keys",
	}
	for _, tc := range []struct {
		name string
		w, h int
		pct  float64
		done bool
	}{
		{"narrow-40x20", 40, 20, 0.33, false},
		{"standard-80x24", 80, 24, 0.66, false},
		{"wide-120x40", 120, 40, 1.0, true},
	} {
		m := previewModel(t, tc.w, tc.h, tc.pct, logs, tc.done)
		var b strings.Builder
		fmt.Fprintf(&b, "\n=== %s (declared %dx%d) ===\n", tc.name, tc.w, tc.h)
		for i, line := range strings.Split(m.View(), "\n") {
			fmt.Fprintf(&b, "%2d |%s|\n", i, line)
		}
		t.Log(b.String())
	}
}
