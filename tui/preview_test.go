package tui

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"testing"

	"github.com/charmbracelet/bubbles/progress"
	"github.com/charmbracelet/bubbles/spinner"
	"github.com/charmbracelet/bubbles/viewport"
	tea "github.com/charmbracelet/bubbletea"

	"blank/app"
)

func previewModel(t *testing.T, w, h int, pct float64, rows []activityRow, done bool) model {
	t.Helper()
	ctx, cancel := context.WithCancel(context.Background())
	t.Cleanup(cancel)
	vp := viewport.New(0, 0)
	bar := progress.New(progress.WithScaledGradient(rosePineLove, rosePinePine))
	sp := spinner.New(spinner.WithSpinner(spinner.MiniDot))
	sp.Style = busyStyle
	bar.EmptyColor = rosePineHighlightMed
	m := model{
		progress: bar,
		spinner:  sp,
		viewport: vp,
		ctx:      ctx,
		cancel:   cancel,
		rows:     rows,
	}
	m.completed = done
	m.engineSettled = done
	updated, _ := m.Update(tea.WindowSizeMsg{Width: w, Height: h})
	m = updated.(model)
	m.progress.SetPercent(pct)
	m.viewport.SetContent(m.activityView())
	m.viewport.GotoBottom()
	return m
}

// demoRows builds one row per stage in turn, so a layout test that sweeps
// widths also sweeps everything the state column can hold.
func demoRows(texts ...string) []activityRow {
	stages := []app.Stage{app.StageStart, app.StageDone, app.StageFailed}
	rows := make([]activityRow, len(texts))
	for i, text := range texts {
		rows[i] = activityRow{path: text, text: text, stage: stages[i%len(stages)]}
	}
	return rows
}

func TestPreviewRender(t *testing.T) {
	logs := demoRows(
		"Overwriting /Users/dev/Documents/secret-report.pdf",
		"Overwriting /Users/dev/Documents/keys/id_rsa",
		"Deleting directory /Users/dev/Documents/keys",
	)
	deepText := make([]string, 40)
	for i := range deepText {
		deepText[i] = fmt.Sprintf("Overwriting /Users/dev/Documents/archive/report-%03d.pdf", i)
	}
	deep := demoRows(deepText...)
	for _, tc := range []struct {
		name string
		w, h int
		pct  float64
		logs []activityRow
		done bool
	}{
		{"narrow-40x20", 40, 20, 0.33, logs, false},
		{"standard-80x24", 80, 24, 0.66, logs, false},
		{"wide-120x40", 120, 40, 1.0, logs, true},
		{"scrolled-80x24", 80, 24, 0.5, deep, false},
	} {
		logFrame(t, tc.name, tc.w, tc.h, previewModel(t, tc.w, tc.h, tc.pct, tc.logs, tc.done).View())
	}
}

// The state machine tests prove the verdict. Only a render shows whether the
// wording carrying it still fits the screen.
func TestPreviewFailedFrame(t *testing.T) {
	rows := demoRows(
		"Overwriting /data/locked/held.bin",
		"Overwriting /data/notes.txt",
		"Deleting directory /data/locked",
	)
	survived := &app.IncompleteRun{Failures: []app.Failure{
		{Path: "/data/locked/held.bin", Err: errors.New("permission denied")},
		{Path: "/data/locked", Err: errors.New("directory not empty")},
	}}
	for _, tc := range []struct {
		name string
		w, h int
	}{
		{"failed-narrowest", 12, 20},
		{"failed-standard", 80, 24},
	} {
		var cur tea.Model = previewModel(t, tc.w, tc.h, 1, rows, true)
		cur, _ = cur.Update(engineDoneMsg{err: survived})
		logFrame(t, tc.name, tc.w, tc.h, cur.(model).View())
	}
}

func logFrame(t *testing.T, name string, w, h int, view string) {
	t.Helper()
	var b strings.Builder
	fmt.Fprintf(&b, "\n=== %s (declared %dx%d) ===\n", name, w, h)
	for i, line := range strings.Split(view, "\n") {
		fmt.Fprintf(&b, "%2d |%s|\n", i, line)
	}
	t.Log(b.String())
}
