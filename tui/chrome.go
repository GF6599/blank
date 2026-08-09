package tui

import (
	"fmt"
	"strings"

	"github.com/charmbracelet/lipgloss"
	"github.com/charmbracelet/x/ansi"
)

const (
	rosePineMuted        = "#6e6a86"
	rosePineSubtle       = "#908caa"
	rosePineText         = "#e0def4"
	rosePineLove         = "#eb6f92"
	rosePineGold         = "#f6c177"
	rosePinePine         = "#31748f"
	rosePineFoam         = "#9ccfd8"
	rosePineIris         = "#c4a7e7"
	rosePineHighlightMed = "#403d52"
)

var (
	colorMarker = lipgloss.Color(rosePineGold)
	colorMarked = lipgloss.Color(rosePineIris)
	colorBorder = lipgloss.Color(rosePineHighlightMed)
	colorText   = lipgloss.Color(rosePineText)
	colorMuted  = lipgloss.Color(rosePineMuted)
	colorSubtle = lipgloss.Color(rosePineSubtle)
	colorFailed = lipgloss.Color(rosePineLove)
	colorBusy   = lipgloss.Color(rosePineFoam)
)

// The single corner lever. Every border and every hand-built rule reads it.
var panelBorder = lipgloss.NormalBorder()

var (
	markerStyle = lipgloss.NewStyle().Foreground(colorMarker)
	markedStyle = lipgloss.NewStyle().Foreground(colorMarked)
	borderStyle = lipgloss.NewStyle().Foreground(colorBorder)
	rowStyle    = lipgloss.NewStyle().Foreground(colorText)
	statusStyle = lipgloss.NewStyle().Foreground(colorSubtle)
	hintStyle   = lipgloss.NewStyle().Foreground(colorMuted)
	thumbStyle  = lipgloss.NewStyle().Foreground(colorMuted)
	failedStyle = lipgloss.NewStyle().Foreground(colorFailed)
	busyStyle   = lipgloss.NewStyle().Foreground(colorBusy)
)

// An ASCII mark, because a target that survived is the one row an operator must
// be able to read on any terminal and in any font.
const failedMark = "!"

// figlet -f ANSI_Shadow "blank", 6 rows by 42 columns.
var markArt = []string{
	"██████╗ ██╗      █████╗ ███╗   ██╗██╗  ██╗",
	"██╔══██╗██║     ██╔══██╗████╗  ██║██║ ██╔╝",
	"██████╔╝██║     ███████║██╔██╗ ██║█████╔╝ ",
	"██╔══██╗██║     ██╔══██║██║╚██╗██║██╔═██╗ ",
	"██████╔╝███████╗██║  ██║██║ ╚████║██║  ██╗",
	"╚═════╝ ╚══════╝╚═╝  ╚═╝╚═╝  ╚═══╝╚═╝  ╚═╝",
}

const (
	markRows      = 6
	markArtCols   = 42
	markMinWidth  = markArtCols
	markMinHeight = 22
	wordmark      = "blank"
)

// Below either gate the art wraps into unreadable fragments.
func markVisible(width, height int) bool {
	return width >= markMinWidth && height >= markMinHeight
}

func renderMark(width, height int) string {
	if !markVisible(width, height) {
		return markedStyle.Render(wordmark)
	}
	rows := make([]string, markRows)
	for i := range markArt {
		rows[i] = markedStyle.Render(markArt[i])
	}
	return strings.Join(rows, "\n")
}

// The gap after the label is budgeted before the label is admitted, so a title
// that only just fits cannot weld itself to the corner.
func frameTop(width int, title string) string {
	if width < 2 {
		return borderStyle.Render(strings.Repeat(panelBorder.Top, max(width, 0)))
	}
	plain := panelBorder.TopLeft + strings.Repeat(panelBorder.Top, width-2) + panelBorder.TopRight
	if title == "" {
		return borderStyle.Render(plain)
	}
	// Corners, the lead rune, a cell either side of the label, and at least one
	// cell of fill. Without the fill the rule ends "title┐".
	overhead := 2 + 1 + 1 + lipgloss.Width(title) + 1
	if width < overhead+1 {
		return borderStyle.Render(plain)
	}
	rule := panelBorder.TopLeft + panelBorder.Top + " " + title + " " +
		strings.Repeat(panelBorder.Top, width-overhead) + panelBorder.TopRight
	return borderStyle.Render(rule)
}

// The top is left open, because frameTop draws it with the title cut in.
var panelSides = lipgloss.NewStyle().
	Border(panelBorder, false, true, true, true).
	BorderForeground(colorBorder)

const (
	controlsFull  = "[jk] scroll  [q] quit"
	controlsShort = "[q] quit"
	barMinWidth   = 10
	controlsGap   = 2
)

// A narrow terminal drops the hint rather than overrunning the content column.
func controlsFor(contentWidth int) string {
	for _, c := range []string{controlsFull, controlsShort} {
		if contentWidth >= barMinWidth+controlsGap+lipgloss.Width(c) {
			return c
		}
	}
	return ""
}

func reservedForControls(contentWidth int) int {
	c := controlsFor(contentWidth)
	if c == "" {
		return 0
	}
	return lipgloss.Width(c) + controlsGap
}

// renderControls reads the text it is given, so the width math has one source.
func renderControls(plain string) string {
	var b strings.Builder
	for _, r := range plain {
		if r == '[' || r == ']' {
			b.WriteString(markerStyle.Render(string(r)))
			continue
		}
		b.WriteString(hintStyle.Render(string(r)))
	}
	return b.String()
}

const markerCells = 6 // "[ ", the state cell, " ", and " ]"

// The bracket cells and the state cell are drawn on every row and blanked when
// they have nothing to say, so a label never shifts sideways as the marker moves
// down the list or a target settles. The caller owns the state cell, which must
// measure one column.
func markerRow(width int, state, label string, current bool) string {
	inner := width - markerCells
	if inner < 1 {
		return strings.Repeat(" ", max(width, 0))
	}
	label = clampTail(label, inner)
	label += strings.Repeat(" ", max(inner-lipgloss.Width(label), 0))

	lead, tail := " ", " "
	body := rowStyle.Render(label)
	if current {
		lead, tail = "[", "]"
		body = markedStyle.Render(label)
	}
	return markerStyle.Render(lead) + " " + state + " " + body + " " + markerStyle.Render(tail)
}

const (
	// Reserved whether or not the panel scrolls, so no label is re-clamped at
	// the moment the history outgrows the panel.
	scrollbarCells = 2

	// A drawn track would sit against the panel border, which is already a light
	// vertical, and the two render as one thick frame edge.
	scrollThumb = "█"
)

// scrollbar returns exactly height cells, one for each visible row. The lit run
// marks where those rows sit in the history.
func scrollbar(height, total, offset int) []string {
	if height <= 0 {
		return nil
	}
	thumb, top := 0, 0
	if total > height {
		thumb = max(height*height/total, 1)
		offset = min(max(offset, 0), total-height)
		// The travel is the scrollable distance, not the whole history. A divisor
		// of total stops the thumb short of the last row at the end of a run,
		// which reads as more history below.
		top = offset * (height - thumb) / (total - height)
	}
	rows := make([]string, height)
	for i := range rows {
		cell := " "
		if i >= top && i < top+thumb {
			cell = thumbStyle.Render(scrollThumb)
		}
		rows[i] = strings.Repeat(" ", scrollbarCells-1) + cell
	}
	return rows
}

const ellipsis = "..."

// The filename identifies the target. The leading directories rarely do.
// A wide character costs one rune and two cells, so the budget counts cells and
// the cut runs through the same library that measures them.
func clampTail(s string, width int) string {
	if width <= 0 {
		return ""
	}
	cells := lipgloss.Width(s)
	if cells <= width {
		return s
	}
	keep, prefix := width, ""
	if lead := lipgloss.Width(ellipsis); width > lead {
		keep, prefix = width-lead, ellipsis
	}
	// TruncateLeft keeps a grapheme whole, so a cut landing inside a wide
	// character returns more cells than it was asked to leave. Drop one more
	// until the result fits.
	out := ansi.TruncateLeft(s, cells-keep, prefix)
	for drop := cells - keep + 1; lipgloss.Width(out) > width && drop <= cells; drop++ {
		out = ansi.TruncateLeft(s, drop, prefix)
	}
	return out
}

// A label reads from its start, unlike a path.
func clampHead(s string, width int) string {
	if width <= 0 {
		return ""
	}
	return ansi.Truncate(s, width, "")
}

// runOutcome is how far the run got, as the engine reported it.
type runOutcome int

const (
	runActive runOutcome = iota
	runComplete
	runCancelled
	runFailed
)

// statusLine reads "complete" for runComplete alone, so this line and the exit
// code are built from the same verdict and cannot disagree. A failure with
// nothing to count still says so, because an engine error that carries no list
// is still an engine error.
func statusLine(o runOutcome, survivors int) string {
	switch o {
	case runComplete:
		return "blanking complete"
	case runCancelled:
		return "blanking cancelled"
	case runFailed:
		if survivors < 1 {
			return "blanking failed"
		}
		noun := "targets"
		if survivors == 1 {
			noun = "target"
		}
		return fmt.Sprintf("blanking failed, %d %s survived", survivors, noun)
	default:
		return "blanking"
	}
}

// The survivor count is the one line an operator must not skim past, so it
// carries the colour the `!` marks already use.
func statusStyleFor(o runOutcome) lipgloss.Style {
	if o == runFailed {
		return failedStyle
	}
	return statusStyle
}
