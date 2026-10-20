use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::symbols::border;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::engine::Stage;
use crate::plan::Incomplete;
use crate::terminal::escape_path;

const LOVE: Color = Color::Rgb(0xeb, 0x6f, 0x92);
const GOLD: Color = Color::Rgb(0xf6, 0xc1, 0x77);
const PINE: Color = Color::Rgb(0x31, 0x74, 0x8f);
const FOAM: Color = Color::Rgb(0x9c, 0xcf, 0xd8);
const IRIS: Color = Color::Rgb(0xc4, 0xa7, 0xe7);
const TEXT: Color = Color::Rgb(0xe0, 0xde, 0xf4);
const SUBTLE: Color = Color::Rgb(0x90, 0x8c, 0xaa);
const MUTED: Color = Color::Rgb(0x6e, 0x6a, 0x86);
const HIGHLIGHT_MED: Color = Color::Rgb(0x40, 0x3d, 0x52);

/// The one corner lever. Every rule the view draws reads its runes from here.
const BORDER: border::Set = border::PLAIN;

/// The marker is chrome saying "this one", and the label it marks is the item itself.
const MARKER: Color = GOLD;
const MARKED: Color = IRIS;

const PADDING: u16 = 2;
const MAX_WIDTH: u16 = 80;
/// Every wording the chrome can show survives this width, and below it nothing is left to cut
/// that still names the outcome.
const MIN_CONTENT_WIDTH: u16 = 8;

/// Rows the panel keeps. Every frame draws the rows in view from this history, and the
/// manifest holds every path for anyone who needs all of them.
const HISTORY: usize = 200;

/// figlet -f ANSI_Shadow "blank", 6 rows by 42 columns.
const MARK_ART: [&str; 6] = [
    "██████╗ ██╗      █████╗ ███╗   ██╗██╗  ██╗",
    "██╔══██╗██║     ██╔══██╗████╗  ██║██║ ██╔╝",
    "██████╔╝██║     ███████║██╔██╗ ██║█████╔╝ ",
    "██╔══██╗██║     ██╔══██║██║╚██╗██║██╔═██╗ ",
    "██████╔╝███████╗██║  ██║██║ ╚████║██║  ██╗",
    "╚═════╝ ╚══════╝╚═╝  ╚═╝╚═╝  ╚═══╝╚═╝  ╚═╝",
];
const MARK_COLUMNS: u16 = 42;
/// Below either size the art leaves the panel too few rows, so the word stands in for it.
const MARK_MIN_HEIGHT: u16 = 22;
const WORDMARK: &str = "blank";

const PANEL_TITLE: &str = "activity";
const CONTROLS: [&str; 3] = [
    "[/] filter  [jk] scroll  [q] quit",
    "[jk] scroll  [q] quit",
    "[q] quit",
];
const BAR_MIN_WIDTH: u16 = 10;
const CONTROLS_GAP: u16 = 2;
const FILTER_PROMPT: &str = "filter: ";

/// "[ ", the state cell, " ", and " ]".
const MARKER_CELLS: u16 = 6;
/// A cell between the left rule and the bracket, which would otherwise weld to it.
const ROW_INDENT: u16 = 1;
/// Reserved whether or not the panel scrolls, so no label is cut again at the moment the
/// history outgrows the panel.
const SCROLLBAR_CELLS: u16 = 2;
const SCROLL_THUMB: &str = "█";
const ELLIPSIS: &str = "...";

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
        let width = area
            .width
            .saturating_sub(PADDING * 2)
            .clamp(MIN_CONTENT_WIDTH, MAX_WIDTH)
            .min(area.width.saturating_sub(PADDING));
        let x = area.x + PADDING.min(area.width);
        let mark = if width >= MARK_COLUMNS && area.height >= MARK_MIN_HEIGHT {
            MARK_ART.len() as u16
        } else {
            1
        };
        // A blank row, the mark, a blank, the bar, the status, a blank, and the panel.
        let panel_top = area.y + 1 + mark + 1 + 2 + 1;
        let panel_height = area.bottom().saturating_sub(panel_top);
        let buffer = frame.buffer_mut();
        self.draw_mark(buffer, x, area.y + 1, width, mark);
        let bar_row = area.y + 1 + mark + 1;
        self.draw_bar(buffer, x, bar_row, width);
        let cursor = self.draw_status(buffer, x, bar_row + 1, width);
        self.draw_panel(buffer, Rect::new(x, panel_top, width, panel_height));
        if let Some(cursor) = cursor {
            frame.set_cursor_position(cursor);
        }
    }

    fn draw_mark(&self, buffer: &mut Buffer, x: u16, y: u16, width: u16, rows: u16) {
        let style = Style::new().fg(MARKED);
        if rows == 1 {
            put_within(buffer, x, y, WORDMARK, usize::from(width), style);
            return;
        }
        for (offset, line) in (0..).zip(MARK_ART) {
            put_within(buffer, x, y + offset, line, usize::from(width), style);
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
            let first = x + width - cells(controls);
            for (column, character) in (first..).zip(controls.chars()) {
                let color = if matches!(character, '[' | ']') {
                    MARKER
                } else {
                    MUTED
                };
                put(
                    buffer,
                    column,
                    y,
                    character.to_string(),
                    Style::new().fg(color),
                );
            }
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
        if area.height < 3 || area.width < 2 {
            self.panel_rows = 1;
            return;
        }
        let border_style = Style::new().fg(HIGHLIGHT_MED);
        let heading = self.heading();
        put(
            buffer,
            area.x,
            area.y,
            frame_top(area.width, &heading),
            border_style,
        );
        let body = area.height - 2;
        self.panel_rows = usize::from(body);
        let shown = self.shown();
        let first = self.first_in_view();
        let newest = self.rows.len().checked_sub(1);
        let inner = area.width - 2;
        let row_width = inner.saturating_sub(SCROLLBAR_CELLS + ROW_INDENT);
        let thumb = scrollbar(usize::from(body), shown.len(), first);
        for line in 0..body {
            let y = area.y + 1 + line;
            put(buffer, area.x, y, BORDER.vertical_left, border_style);
            put(
                buffer,
                area.right() - 1,
                y,
                BORDER.vertical_right,
                border_style,
            );
            if let Some(&(index, row)) = shown.get(first + usize::from(line)) {
                let current = Some(index) == newest && self.report.is_none();
                self.draw_row(buffer, area.x + 1 + ROW_INDENT, y, row_width, row, current);
            }
            if thumb.contains(&usize::from(line)) && inner >= SCROLLBAR_CELLS {
                let column = area.x + 1 + inner - 1;
                put(buffer, column, y, SCROLL_THUMB, Style::new().fg(MUTED));
            }
        }
        let bottom = format!(
            "{}{}{}",
            BORDER.bottom_left,
            BORDER.horizontal_bottom.repeat(usize::from(inner)),
            BORDER.bottom_right
        );
        put(buffer, area.x, area.bottom() - 1, bottom, border_style);
    }

    /// The bracket cells and the state cell are drawn on every row and left blank when they
    /// have nothing to say, so a label never shifts as the marker moves or a target settles.
    fn draw_row(&self, buffer: &mut Buffer, x: u16, y: u16, width: u16, row: &Row, current: bool) {
        let (state, state_color) = match row.stage {
            Stage::Start => (SPINNER[self.spinner], FOAM),
            Stage::Done => (" ", TEXT),
            Stage::Failed => (FAILED_MARK, LOVE),
        };
        if width < MARKER_CELLS + 1 {
            put_within(
                buffer,
                x,
                y,
                state,
                usize::from(width),
                Style::new().fg(state_color),
            );
            return;
        }
        let inner = width - MARKER_CELLS;
        let (lead, tail, label_color) = if current {
            ("[", "]", MARKED)
        } else {
            (" ", " ", TEXT)
        };
        let label = clamp_tail(&row.text, inner, ELLIPSIS);
        put(buffer, x, y, lead, Style::new().fg(MARKER));
        put(buffer, x + 2, y, state, Style::new().fg(state_color));
        put(buffer, x + 4, y, &label, Style::new().fg(label_color));
        put(buffer, x + width - 1, y, tail, Style::new().fg(MARKER));
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

/// The top rule with the title cut in. The gap after the title is budgeted before the title
/// is admitted, so a title that only just fits cannot weld itself to the corner.
fn frame_top(width: u16, title: &str) -> String {
    let width = usize::from(width);
    let rule = |count: usize| BORDER.horizontal_top.repeat(count);
    // The corners, the lead rune, a cell either side of the title, and one cell of fill.
    let overhead = 2 + 1 + 1 + title.width() + 1 + 1;
    if title.is_empty() || width < overhead {
        return format!("{}{}{}", BORDER.top_left, rule(width - 2), BORDER.top_right);
    }
    format!(
        "{}{} {title} {}{}",
        BORDER.top_left,
        rule(1),
        rule(width - overhead + 1),
        BORDER.top_right
    )
}

/// The rows of the panel that carry the thumb. The travel is the distance the panel can
/// scroll, so the thumb reaches the last row when the newest row is in view.
fn scrollbar(height: usize, total: usize, first: usize) -> std::ops::Range<usize> {
    if total <= height || height == 0 {
        return 0..0;
    }
    let thumb = (height * height / total).max(1);
    let first = first.min(total - height);
    let top = first * (height - thumb) / (total - height);
    top..top + thumb
}

/// The end of a label, because the filename identifies a target and its leading directories
/// rarely do. Counted in cells, because a wide character is one character and two cells.
fn clamp_tail(text: &str, width: u16, prefix: &str) -> String {
    let width = usize::from(width);
    if text.width() <= width {
        return text.to_owned();
    }
    let prefix = if width > prefix.width() { prefix } else { "" };
    let mut kept = Vec::new();
    let mut used = prefix.width();
    for character in text.chars().rev() {
        let cell = character.width().unwrap_or(0);
        if used + cell > width {
            break;
        }
        used += cell;
        kept.push(character);
    }
    prefix.chars().chain(kept.into_iter().rev()).collect()
}

fn cells(text: &str) -> u16 {
    u16::try_from(text.width()).unwrap_or(u16::MAX)
}

fn blend(from: Color, to: Color, share: f64) -> Color {
    let (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) = (from, to) else {
        return from;
    };
    let mix = |a: u8, b: u8| (f64::from(a) + (f64::from(b) - f64::from(a)) * share).round() as u8;
    Color::Rgb(mix(r1, r2), mix(g1, g2), mix(b1, b2))
}

/// Writes `text` at a cell, and nothing when the cell lies outside the screen. The rows above
/// the panel sit at fixed offsets, so a short terminal puts some of them below its last row.
fn put(buffer: &mut Buffer, x: u16, y: u16, text: impl AsRef<str>, style: Style) -> (u16, u16) {
    put_within(buffer, x, y, text, usize::MAX, style)
}

fn put_within(
    buffer: &mut Buffer,
    x: u16,
    y: u16,
    text: impl AsRef<str>,
    width: usize,
    style: Style,
) -> (u16, u16) {
    let area = buffer.area;
    if y >= area.bottom() || x >= area.right() {
        return (x, y);
    }
    buffer.set_stringn(x, y, text, width, style)
}
