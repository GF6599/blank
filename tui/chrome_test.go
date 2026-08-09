package tui

import (
	"strings"
	"testing"

	"github.com/charmbracelet/lipgloss"
)

// A framed panel keeps its declared width and cuts its title into the top rule.
// Given a title and a width, when the top rule renders, then it spans exactly
// that width with the label followed by fill.
// Why it matters: a rule that under-budgets its own gap tears the frame at the
// widths where the title nearly fits, which fixed-width tests walk straight past.
func TestFrameTopKeepsWidthAndCutsInTheTitle(t *testing.T) {
	// Given: every width a terminal might hand us, swept rather than sampled.
	for w := 12; w <= 160; w++ {
		// When: the top rule renders with a title.
		got := frameTop(w, "activity")

		// Then: it occupies exactly the width asked for.
		if lipgloss.Width(got) != w {
			t.Fatalf("width %d: rule measured %d\n%s", w, lipgloss.Width(got), got)
		}
		// And: no gap is ever stranded inside the rule.
		if strings.Contains(got, "─ ─") {
			t.Fatalf("width %d: stranded gap inside the rule: %s", w, got)
		}
		// And: when the label survives it carries exactly one cell either side.
		if strings.Contains(got, "activity") && !strings.Contains(got, "─ activity ─") {
			t.Fatalf("width %d: title not cut cleanly into the rule: %s", w, got)
		}
	}
}

// A title-less rule renders as plain fill with no orphan gap.
// Given no title, when the rule renders, then it is corners plus fill only.
// Why it matters: dropping the label but keeping its lead leaves "┌─ ────┐",
// which reads as a rendering fault rather than an empty panel.
func TestFrameTopWithoutTitleHasNoOrphanGap(t *testing.T) {
	// Given / When: a rule with no label.
	got := frameTop(40, "")

	// Then: it is exactly as wide as asked, and carries no space at all.
	if lipgloss.Width(got) != 40 {
		t.Fatalf("measured %d: %s", lipgloss.Width(got), got)
	}
	if strings.Contains(got, " ") {
		t.Fatalf("orphan gap in a title-less rule: %s", got)
	}
}

// The mark degrades to the plain wordmark rather than shredding the block art.
// Given a terminal below the gate, when the mark renders, then it is one line.
// Why it matters: ANSI_Shadow runs 42 columns, so a narrow window wraps it into
// unreadable fragments, which is worse than having no mark at all.
func TestMarkDegradesBelowTheGate(t *testing.T) {
	for _, tc := range []struct {
		name   string
		w, h   int
		blocky bool
	}{
		{"too narrow", markMinWidth - 1, 40, false},
		{"too short", 120, markMinHeight - 1, false},
		{"room for it", markMinWidth, markMinHeight, true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			// When: the mark renders at that size.
			got := renderMark(tc.w, tc.h)
			rows := strings.Count(got, "\n") + 1

			// Then: it is block art only when there is room for it.
			if tc.blocky && rows != markRows {
				t.Fatalf("expected %d rows of block art, got %d:\n%s", markRows, rows, got)
			}
			if !tc.blocky {
				if rows != 1 {
					t.Fatalf("expected a single-line wordmark, got %d rows:\n%s", rows, got)
				}
				if !strings.Contains(got, "blank") {
					t.Fatalf("wordmark lost the name: %q", got)
				}
			}
			// And: it never overruns the width it was given.
			for _, line := range strings.Split(got, "\n") {
				if lipgloss.Width(line) > tc.w {
					t.Fatalf("mark overran %d columns: %q", tc.w, line)
				}
			}
		})
	}
}

// The mark is the block art alone, with nothing wrapped around it.
// Given a terminal above the gate and one below it, when the mark renders, then
// neither form carries a brace.
// Why it matters: the brace was a two-part lockup, drawn as box runes beside the
// art and as literal braces in the degraded wordmark. Removing one and not the
// other leaves the small terminal braced and the large one bare.
func TestMarkCarriesNoBraces(t *testing.T) {
	braces := []string{"{", "}", "╭", "╮", "╰", "╯", "┤", "├", "│"}
	for _, tc := range []struct {
		name string
		w, h int
	}{
		{"block art", markMinWidth, markMinHeight},
		{"degraded wordmark", 20, 10},
	} {
		t.Run(tc.name, func(t *testing.T) {
			// When: the mark renders.
			got := renderMark(tc.w, tc.h)

			// Then: no brace rune survives in either form.
			for _, b := range braces {
				if strings.Contains(got, b) {
					t.Fatalf("mark still carries %q:\n%s", b, got)
				}
			}
		})
	}
}

// The bracket marker reserves its cells on every row, current or not.
// Given rows where only one is current, when they render, then all share a width
// and the brackets sit in the same columns throughout.
// Why it matters: a marker that only occupies space when active shifts every
// label sideways as it moves, which reads as the list twitching.
func TestMarkerReservesItsCellsOnEveryRow(t *testing.T) {
	// Given: one current row and one that is not, at the same width.
	const w = 40
	current := markerRow(w, "overwriting a.txt", true)
	other := markerRow(w, "overwriting a.txt", false)

	// Then: both measure the same, so nothing shifts as the marker moves.
	if lipgloss.Width(current) != lipgloss.Width(other) {
		t.Fatalf("row widths differ: current=%d other=%d", lipgloss.Width(current), lipgloss.Width(other))
	}
	// And: the current row carries the pair, the other reserves blanks.
	if !strings.Contains(current, "[") || !strings.Contains(current, "]") {
		t.Fatalf("current row lost its marker: %q", current)
	}
	if strings.Contains(other, "[") || strings.Contains(other, "]") {
		t.Fatalf("non-current row drew a marker: %q", other)
	}
	// And: a long label is clamped rather than overflowing the row.
	long := markerRow(w, strings.Repeat("x", 200), true)
	if lipgloss.Width(long) != w {
		t.Fatalf("long label overran the row: measured %d", lipgloss.Width(long))
	}
}

// Every block in the layout shares one content column.
// Given a terminal size, when the whole view renders, then the progress row, the
// title rule and every panel row end in the same column.
// Why it matters: a row that under-budgets its own width only overruns at the
// sizes where the content nearly fits, so three fixed widths walk past it.
func TestLayoutSharesOneContentColumn(t *testing.T) {
	logs := []string{"Overwriting /a/b/c.txt", "Deleting directory /a/b"}
	// Given: a sweep across terminal sizes rather than a few samples. Height is
	// swept too, because the mark degrades on a short terminal and the viewport
	// has to give back the rows it stops using.
	for w := 12; w <= 140; w++ {
		h := 12 + w%30
		m := previewModel(t, w, h, 0.5, logs, false)

		// Then: the view fits the rows it was given, so the panel is never cut off.
		if got := len(strings.Split(m.View(), "\n")); got > h {
			t.Fatalf("terminal %dx%d: view rendered %d rows and overran the screen", w, h, got)
		}

		// When: the view renders.
		var frame []int
		inPanel := false
		for _, line := range strings.Split(m.View(), "\n") {
			trimmed := strings.TrimRight(line, " ")
			if trimmed == "" {
				continue
			}
			// Then: no row overruns the terminal it was drawn for.
			if got := lipgloss.Width(trimmed); got > w {
				t.Fatalf("terminal %d: row %q measured %d and overran the screen", w, trimmed, got)
			}
			// The panel starts at its top rule. The block art also uses box
			// runes, so anything above that rule is not part of the frame.
			if strings.HasPrefix(strings.TrimLeft(trimmed, " "), panelBorder.TopLeft) {
				inPanel = true
			}
			if inPanel {
				frame = append(frame, lipgloss.Width(trimmed))
			}
		}

		// And: every row of the framed panel ends in the same column, so the
		// frame cannot tear at a width where the title nearly fits.
		if len(frame) < 2 {
			t.Fatalf("terminal %d: expected a framed panel, found %d rows", w, len(frame))
		}
		for i, got := range frame {
			if got != frame[0] {
				t.Fatalf("terminal %d: frame row %d measured %d, top rule measured %d", w, i, got, frame[0])
			}
		}
	}
}

// The view is a compact inline block rather than a full screen.
// Given two terminals of one width and very different heights, when the view
// renders, then both occupy the same number of rows.
// Why it matters: an inline program repaints only the rows it drew last, and the
// renderer drops lines off the top when the view is taller than the terminal, so
// a view that grows to fill the screen loses its mark on every frame.
func TestViewStaysCompactWhateverTheTerminalHeight(t *testing.T) {
	logs := []string{"Overwriting /a/b/c.txt"}
	const w = 80

	// Given: two heights, both above the gate where the mark degrades, so the
	// viewport is the only thing that can vary.
	short := previewModel(t, w, 24, 0.5, logs, false)
	tall := previewModel(t, w, 60, 0.5, logs, false)

	shortRows := len(strings.Split(short.View(), "\n"))
	tallRows := len(strings.Split(tall.View(), "\n"))

	// Then: the block is the same size on both.
	if shortRows != tallRows {
		t.Fatalf("view grew with the terminal: %d rows at height 24, %d rows at height 60", shortRows, tallRows)
	}
	// And: it leaves the rest of the screen alone.
	if tallRows >= 60 {
		t.Fatalf("view filled the 60 row terminal (%d rows) instead of staying inline", tallRows)
	}
}

// User-facing chrome carries no em dash.
// Given a completed run, when the status line renders, then it uses no em dash.
// Why it matters: the house mechanics forbid it, and chrome is the copy most
// likely to be written once and never reread.
func TestStatusLineUsesNoEmDash(t *testing.T) {
	for _, done := range []bool{false, true} {
		if got := statusLine(done); strings.ContainsRune(got, '—') {
			t.Fatalf("em dash in status line (done=%v): %q", done, got)
		}
	}
}
