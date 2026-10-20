use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;

use crate::chrome::{
    CONTROLS_GAP, Column, FOAM, HIGHLIGHT_MED, LOVE, Line, MUTED, PINE, SUBTLE, TEXT, blend, cells,
    clamp_tail, draw_controls, draw_mark, draw_panel, panel_rows, put, put_within,
};
use crate::engine::Stage;
use crate::plan::Incomplete;
use crate::terminal::escape_path;

/// Rows the panel keeps. Every frame draws the rows in view from this history, and the
/// manifest holds every path for anyone who needs all of them.
const HISTORY: usize = 200;

const PANEL_TITLE: &str = "activity";
const CONTROLS: [&str; 3] = [
    "[/] filter  [jk] scroll  [q] quit",
    "[jk] scroll  [q] quit",
    "[q] quit",
];
const BAR_MIN_WIDTH: u16 = 10;
const FILTER_PROMPT: &str = "filter: ";

/// An ASCII mark, because a target that survived is the one row an operator must be able to
/// read on any terminal and in any font.
const FAILED_MARK: &str = "!";
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Active,
    Complete,
    Cancelled,
    Failed { survivors: usize },
}

impl Outcome {
    /// The line printed once the screen closes, which is the longest form of the status line.
    pub fn verdict(self) -> String {
        self.wordings().swap_remove(0)
    }

    /// Every length the status line comes in, longest first. Only a run in flight opens with
    /// "blanking" alone, so no cut of another wording can read as a run still going, and each
    /// list ends on a wording that fits `MIN_CONTENT_WIDTH`.
    fn wordings(self) -> Vec<String> {
        let fixed = |words: [&str; 2]| words.map(str::to_owned).to_vec();
        match self {
            Outcome::Active => vec!["blanking".to_owned()],
            Outcome::Complete => fixed(["blanking complete", "complete"]),
            Outcome::Cancelled => fixed(["blanking cancelled", "cancelled"]),
            Outcome::Failed { survivors } => {
                let noun = if survivors == 1 { "target" } else { "targets" };
                vec![
                    format!("blanking failed, {survivors} {noun} survived"),
                    format!("failed, {survivors} survived"),
                    format!("failed {survivors}"),
                    "failed".to_owned(),
                ]
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    /// Stop the run, or close the screen when the run is over.
    Stop,
}

struct Row {
    path: PathBuf,
    text: String,
    stage: Stage,
}

/// The state of the full screen view of a run. It draws from what the engine reported and
/// never from the files, so the screen and the exit code read the same verdict.
pub struct RunView {
    rows: VecDeque<Row>,
    done: u64,
    total: u64,
    report: Option<Result<(), usize>>,
    stopped: bool,
    filter: String,
    filtering: bool,
    /// The first row in view when the operator has scrolled away from the newest one.
    held: Option<usize>,
    /// Rows the panel showed in the last frame, which a half page scroll reads.
    panel_rows: usize,
    spinner: usize,
}

impl Default for RunView {
    fn default() -> RunView {
        RunView::new()
    }
}

impl RunView {
    pub fn new() -> RunView {
        RunView {
            rows: VecDeque::new(),
            done: 0,
            total: 1,
            report: None,
            stopped: false,
            filter: String::new(),
            filtering: false,
            held: None,
            panel_rows: 1,
            spinner: 0,
        }
    }

    /// Adds a target the run reached, or settles one the panel holds. A settle is searched for
    /// from the newest row, because a target settles close behind its start. One the history
    /// already dropped is left alone, so a late settle cannot mark another row.
    pub fn target(&mut self, verb: &str, path: &Path, stage: Stage) {
        if stage == Stage::Start {
            let text = format!("{verb} {}", escape_path(path));
            self.rows.push_back(Row {
                path: path.to_path_buf(),
                text,
                stage,
            });
            if self.rows.len() > HISTORY {
                self.rows.pop_front();
                self.held = self.held.map(|first| first.saturating_sub(1));
            }
            return;
        }
        if let Some(row) = self.rows.iter_mut().rev().find(|row| row.path == path) {
            row.stage = stage;
        }
    }

    pub fn progress(&mut self, done: u64, total: u64) {
        self.done = done;
        self.total = total.max(1);
    }

    pub fn finish(&mut self, report: &Result<(), Incomplete>) {
        self.report = Some(
            report
                .as_ref()
                .copied()
                .map_err(|incomplete| incomplete.survivors().len()),
        );
    }

    pub fn tick(&mut self) {
        self.spinner = (self.spinner + 1) % SPINNER.len();
    }

    pub fn outcome(&self) -> Outcome {
        match self.report {
            Some(Ok(())) => Outcome::Complete,
            Some(Err(_)) | None if self.stopped => Outcome::Cancelled,
            Some(Err(survivors)) => Outcome::Failed { survivors },
            None => Outcome::Active,
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> Action {
        // Ahead of the filter, because this screen is destroying files and the key that stops
        // it must never become text in an input.
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return self.cancel();
        }
        if self.filtering {
            match key.code {
                KeyCode::Enter => self.filtering = false,
                KeyCode::Esc => {
                    self.filtering = false;
                    self.filter.clear();
                }
                KeyCode::Backspace => {
                    self.filter.pop();
                }
                KeyCode::Char(character) => self.filter.push(character),
                _ => {}
            }
            self.held = None;
            return Action::None;
        }
        let half = (self.panel_rows / 2).max(1);
        match key.code {
            KeyCode::Char('q') => return self.cancel(),
            KeyCode::Esc if self.filter.is_empty() => return self.cancel(),
            KeyCode::Esc => {
                self.filter.clear();
                self.held = None;
            }
            KeyCode::Char('/') => self.filtering = true,
            KeyCode::Up | KeyCode::Char('k') => self.scroll_up(1),
            KeyCode::Down | KeyCode::Char('j') => self.scroll_down(1),
            KeyCode::PageUp | KeyCode::Char('b') => self.scroll_up(half),
            KeyCode::PageDown | KeyCode::Char('f') => self.scroll_down(half),
            _ => {}
        }
        Action::None
    }

    /// Marks a run in flight as cancelled. Quitting a run that already settled only closes the
    /// screen, so it never turns a verdict already reached into a cancelled one.
    pub fn cancel(&mut self) -> Action {
        if self.report.is_none() {
            self.stopped = true;
        }
        Action::Stop
    }

    fn shown(&self) -> Vec<(usize, &Row)> {
        let filter = self.filter.to_lowercase();
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| filter.is_empty() || row.text.to_lowercase().contains(&filter))
            .collect()
    }

    fn last_first(&self) -> usize {
        self.shown().len().saturating_sub(self.panel_rows)
    }

    fn first_in_view(&self) -> usize {
        self.held.unwrap_or(usize::MAX).min(self.last_first())
    }

    fn scroll_up(&mut self, rows: usize) {
        self.held = Some(self.first_in_view().saturating_sub(rows));
    }

    /// Reaching the newest row lets the panel follow the run again.
    fn scroll_down(&mut self, rows: usize) {
        let first = self.first_in_view() + rows;
        self.held = (first < self.last_first()).then_some(first);
    }

    /// Every block shares one content column, so they line up on both edges.
    pub fn draw(&mut self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let column = Column::of(area);
        let buffer = frame.buffer_mut();
        let mark = draw_mark(buffer, column, area.y + 1, area.height);
        // A blank row, the mark, a blank, the bar, the status, a blank, and the panel.
        let bar_row = area.y + 1 + mark + 1;
        self.draw_bar(buffer, column.x, bar_row, column.width);
        let cursor = self.draw_status(buffer, column.x, bar_row + 1, column.width);
        let panel_top = bar_row + 3;
        let panel_height = area.bottom().saturating_sub(panel_top);
        self.draw_panel(
            buffer,
            Rect::new(column.x, panel_top, column.width, panel_height),
        );
        if let Some(cursor) = cursor {
            frame.set_cursor_position(cursor);
        }
    }

    /// A narrow terminal drops the key hint rather than overrunning the column.
    fn draw_bar(&self, buffer: &mut Buffer, x: u16, y: u16, width: u16) {
        let controls = CONTROLS
            .into_iter()
            .find(|controls| width >= BAR_MIN_WIDTH + CONTROLS_GAP + cells(controls));
        let reserved = controls.map_or(0, |controls| cells(controls) + CONTROLS_GAP);
        let share = self.done as f64 / self.total as f64;
        let percent = format!(" {:>3}%", (share * 100.0).floor() as u64);
        let track = (width - reserved).saturating_sub(cells(&percent));
        let filled = (f64::from(track) * share).floor() as u16;
        for cell in 0..track {
            let (symbol, color) = if cell < filled {
                (
                    "█",
                    blend(LOVE, PINE, f64::from(cell) / f64::from(filled.max(2) - 1)),
                )
            } else {
                ("░", HIGHLIGHT_MED)
            };
            put(buffer, x + cell, y, symbol, Style::new().fg(color));
        }
        put(buffer, x + track, y, &percent, Style::new().fg(SUBTLE));
        if let Some(controls) = controls {
            draw_controls(buffer, Column { x, width }, y, controls);
        }
    }

    /// The status line, and the filter box beside it while it is open. The box gets the room
    /// the verdict leaves, so a long filter shows its newest characters. Returns where the
    /// cursor belongs while the box is open.
    fn draw_status(&self, buffer: &mut Buffer, x: u16, y: u16, width: u16) -> Option<Position> {
        let outcome = self.outcome();
        let wordings = outcome.wordings();
        let status = wordings
            .iter()
            .find(|wording| cells(wording) <= width)
            .unwrap_or(&wordings[wordings.len() - 1]);
        let color = match outcome {
            Outcome::Failed { .. } => LOVE,
            _ => SUBTLE,
        };
        let used = put_within(
            buffer,
            x,
            y,
            status,
            usize::from(width),
            Style::new().fg(color),
        )
        .0 - x;
        if !self.filtering {
            return None;
        }
        let room = width.saturating_sub(used + 1);
        let prompt = cells(FILTER_PROMPT);
        if room < prompt + 2 {
            return None;
        }
        let text = clamp_tail(&self.filter, room - prompt - 1, "");
        let start = x + width - prompt - cells(&text) - 1;
        put(buffer, start, y, FILTER_PROMPT, Style::new().fg(MUTED));
        put(buffer, start + prompt, y, &text, Style::new().fg(TEXT));
        Some(Position::new(start + prompt + cells(&text), y))
    }

    fn draw_panel(&mut self, buffer: &mut Buffer, area: Rect) {
        self.panel_rows = panel_rows(area);
        let heading = self.heading();
        let shown = self.shown();
        let first = self.first_in_view();
        let newest = self.rows.len().checked_sub(1);
        let lines = shown.iter().skip(first).map(|&(index, row)| {
            let (state, state_color) = match row.stage {
                Stage::Start => (SPINNER[self.spinner], FOAM),
                Stage::Done => (" ", TEXT),
                Stage::Failed => (FAILED_MARK, LOVE),
            };
            Line {
                state,
                state_color,
                label: &row.text,
                current: Some(index) == newest && self.report.is_none(),
            }
        });
        draw_panel(buffer, area, &heading, lines, shown.len(), first);
    }

    /// A filtered panel looks exactly like a short run, so the heading carries the filter and
    /// both counts.
    fn heading(&self) -> String {
        if self.filter.is_empty() {
            return PANEL_TITLE.to_owned();
        }
        format!(
            "{PANEL_TITLE}  {}  {} of {}",
            self.filter,
            self.shown().len(),
            self.rows.len()
        )
    }
}
