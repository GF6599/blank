use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};

use crate::chrome::{
    CONTROLS_GAP, Column, LOVE, Line, MUTED, SUBTLE, TEXT, cells, draw_controls, draw_mark,
    draw_panel, panel_rows, put, split_panes, wrap,
};
use crate::engine::Options;
use crate::guard::Guard;
use crate::pane::{PreviewPane, PreviewRequest, PreviewResult};
use crate::plan::{Plan, short_size};
use crate::terminal::{escape, escape_path};

const YES: &str = "yes";
/// The question at each width, longest first.
const PROMPTS: [&str; 3] = [
    "Type yes and press enter to destroy them: ",
    "Type yes, then enter: ",
    "yes, then enter: ",
];
const CONTROLS: [&str; 2] = ["[jk] move  [q] quit", "[q] quit"];
const TITLE: &str = "targets";
/// The header lines past this many push the tree off a small screen.
const HEADER_LINES: usize = 6;
const FAILED_MARK: &str = "!";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Waiting,
    /// The whole word yes, typed and entered.
    Destroy,
    /// Leave, and destroy nothing.
    Quit,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowKind {
    File,
    Symlink,
    Special,
    Directory,
    /// A directory the walk could not list. It survives the run with every directory above it.
    Unreadable,
}

/// A row of the tree as one frame draws it.
struct ShownRow {
    label: String,
    detail: Option<(String, Color)>,
    state: (&'static str, Color),
}

struct Row<'p> {
    path: &'p Path,
    depth: usize,
    kind: RowKind,
    /// The size of a file, or of every file under a directory.
    bytes: Option<u64>,
    /// A file with a hard link outside the selection.
    linked: bool,
}

/// The screen before a run with the interface: what the run will destroy, every target as a
/// tree, the preview of the row under the cursor, and the question. Nothing goes on until the
/// whole word yes is typed and entered.
pub struct Review<'p> {
    header: Vec<(String, Color)>,
    rows: Vec<Row<'p>>,
    cursor: usize,
    first: usize,
    panel_rows: usize,
    answer: String,
    pane: PreviewPane,
    allow_hard_links: bool,
}

impl<'p> Review<'p> {
    pub fn new(plan: &'p Plan, guard: Arc<Guard>, options: &Options) -> Review<'p> {
        Review {
            header: header(plan, options),
            rows: tree(plan),
            allow_hard_links: options.allow_hard_links,
            cursor: 0,
            first: 0,
            panel_rows: 1,
            answer: String::new(),
            pane: PreviewPane::new(guard, "the run destroys"),
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> Answer {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Answer::Quit;
        }
        let page = self.panel_rows.max(1);
        match key.code {
            KeyCode::Enter if self.answer.eq_ignore_ascii_case(YES) => return Answer::Destroy,
            // A wrong answer is cleared, so the operator types the word again from the start.
            KeyCode::Enter => self.answer.clear(),
            KeyCode::Char('q') => return Answer::Quit,
            KeyCode::Esc if self.answer.is_empty() => return Answer::Quit,
            KeyCode::Esc => self.answer.clear(),
            KeyCode::Backspace => {
                self.answer.pop();
            }
            // Only the letters of the answer reach it, so the keys that move through the tree
            // never become part of it.
            KeyCode::Char(character)
                if YES.contains(character.to_ascii_lowercase())
                    && self.answer.len() <= YES.len() =>
            {
                self.answer.push(character);
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_cursor(1, 0),
            KeyCode::Char('k') | KeyCode::Up => self.move_cursor(0, 1),
            KeyCode::PageDown => self.move_cursor(page, 0),
            KeyCode::PageUp => self.move_cursor(0, page),
            KeyCode::Home => self.move_cursor(0, usize::MAX),
            KeyCode::End => self.move_cursor(usize::MAX, 0),
            _ => {}
        }
        Answer::Waiting
    }

    fn move_cursor(&mut self, down: usize, up: usize) {
        let last = self.rows.len().saturating_sub(1);
        self.cursor = self
            .cursor
            .saturating_add(down)
            .saturating_sub(up)
            .min(last);
    }

    /// The preview read the cursor needs, when it reached another row.
    pub fn work(&mut self) -> Option<PreviewRequest> {
        let path = self.rows.get(self.cursor).map(|row| row.path);
        self.pane.follow(path)
    }

    pub fn finished(&mut self, result: PreviewResult) {
        self.pane.finished(result);
    }

    /// Every block shares one content column, the way the other screens build their own.
    pub fn draw(&mut self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let column = Column::of(area);
        let buffer = frame.buffer_mut();
        let mark = draw_mark(buffer, column, area.y + 1, area.height);
        let mut y = area.y + 1 + mark + 1;
        for (text, color) in self.header_lines(column.width) {
            put(buffer, column.x, y, text, Style::new().fg(color));
            y += 1;
        }
        let cursor = self.draw_prompt(buffer, column, y);
        let panes_top = y + 2;
        let panes = Rect::new(
            column.x,
            panes_top,
            column.width,
            area.bottom().saturating_sub(panes_top),
        );
        let (list, pane) = split_panes(panes);
        self.draw_tree(buffer, list);
        if let Some(pane) = pane {
            let pending = match self.rows.get(self.cursor).map(|row| row.kind) {
                Some(RowKind::Directory | RowKind::Unreadable) => "counting",
                _ => "reading",
            };
            self.pane.draw(buffer, pane, pending);
        }
        if let Some(cursor) = cursor {
            frame.set_cursor_position(cursor);
        }
    }

    /// Each sentence of the header enters whole or not at all, because a sentence cut where the
    /// screen ends can drop the number that matters, such as what the covers will write.
    fn header_lines(&self, width: u16) -> Vec<(String, Color)> {
        let mut lines = Vec::new();
        for (text, color) in &self.header {
            let wrapped = wrap(text, width);
            if lines.len() + wrapped.len() > HEADER_LINES {
                break;
            }
            lines.extend(wrapped.into_iter().map(|line| (line, *color)));
        }
        lines
    }

    /// The question and the answer typed so far, with the key hint at the right edge. Returns
    /// where the terminal cursor belongs.
    fn draw_prompt(&self, buffer: &mut Buffer, column: Column, y: u16) -> Option<Position> {
        let answer = cells(&self.answer);
        let prompt = PROMPTS
            .into_iter()
            .find(|prompt| cells(prompt) + answer < column.width)?;
        put(buffer, column.x, y, prompt, Style::new().fg(TEXT));
        let start = column.x + cells(prompt);
        put(buffer, start, y, &self.answer, Style::new().fg(LOVE));
        let used = cells(prompt) + answer + 1;
        if let Some(controls) = CONTROLS
            .into_iter()
            .find(|controls| used + CONTROLS_GAP + cells(controls) <= column.width)
        {
            draw_controls(buffer, column, y, controls);
        }
        Some(Position::new(start + answer, y))
    }

    fn draw_tree(&mut self, buffer: &mut Buffer, area: Rect) {
        self.panel_rows = panel_rows(area);
        // A taller screen than the last one shows again the rows above, rather than empty rows
        // below.
        self.first = self
            .first
            .min(self.rows.len().saturating_sub(self.panel_rows));
        if self.cursor < self.first {
            self.first = self.cursor;
        } else if self.cursor >= self.first + self.panel_rows {
            self.first = self.cursor + 1 - self.panel_rows;
        }
        let shown: Vec<ShownRow> = self
            .rows
            .iter()
            .skip(self.first)
            .take(self.panel_rows)
            .map(|row| {
                // Without permission the run leaves a file linked outside intact, so it reads
                // as a survivor. With it, the note stays, because the run destroys the other
                // names too.
                let linked = row.linked.then_some(if self.allow_hard_links {
                    ("linked outside".to_owned(), MUTED)
                } else {
                    ("linked outside".to_owned(), LOVE)
                });
                let survives =
                    row.kind == RowKind::Unreadable || (row.linked && !self.allow_hard_links);
                ShownRow {
                    label: label(row),
                    detail: match row.kind {
                        RowKind::Unreadable => Some(("cannot be read".to_owned(), LOVE)),
                        _ => linked.or_else(|| row.bytes.map(|bytes| (short_size(bytes), MUTED))),
                    },
                    state: if survives {
                        (FAILED_MARK, LOVE)
                    } else {
                        (" ", TEXT)
                    },
                }
            })
            .collect();
        let lines = shown.iter().enumerate().map(|(offset, row)| Line {
            state: row.state.0,
            state_color: row.state.1,
            label: &row.label,
            detail: row
                .detail
                .as_ref()
                .map(|(text, color)| (text.as_str(), *color)),
            current: self.first + offset == self.cursor,
        });
        draw_panel(buffer, area, TITLE, lines, self.rows.len(), self.first);
    }
}

/// A root reads as its whole path, and every row under it as its name, indented by its depth.
fn label(row: &Row<'_>) -> String {
    let suffix = match row.kind {
        RowKind::Directory | RowKind::Unreadable => "/",
        RowKind::Symlink => "@",
        RowKind::File | RowKind::Special => "",
    };
    if row.depth == 0 {
        return format!("{}{suffix}", escape_path(row.path));
    }
    let name = row
        .path
        .file_name()
        .map_or_else(String::new, |name| escape(&name.to_string_lossy()));
    format!("{}{name}{suffix}", "  ".repeat(row.depth))
}

/// What the run will destroy, what the covers will write, and what will survive.
fn header(plan: &Plan, options: &Options) -> Vec<(String, Color)> {
    let mut totals = Vec::new();
    plan.statistics()
        .write_totals(&mut totals, options)
        .expect("writing to memory cannot fail");
    let totals = String::from_utf8(totals).expect("the totals are UTF-8");
    let mut lines = vec![(format!("blank will destroy {}.", plan.summary()), TEXT)];
    lines.extend(totals.lines().enumerate().map(|(index, line)| {
        // The first line holds the totals, and any line after it names what will survive.
        let color = if index == 0 { SUBTLE } else { LOVE };
        (line.to_owned(), color)
    }));
    lines
}

/// Every target and every directory the walk could not list, in the order of a tree, each
/// with its depth below its root, and the size of every file and of every directory's files.
fn tree(plan: &Plan) -> Vec<Row<'_>> {
    let statistics = plan.statistics();
    let linked: HashSet<&Path> = statistics
        .linked
        .iter()
        .map(|path| path.as_path())
        .collect();
    let directories: HashSet<&Path> = plan.directories.iter().map(|path| path.as_path()).collect();
    let mut directory_bytes: HashMap<&Path, u64> = HashMap::new();
    for file in &plan.files {
        let bytes = plan.size(file);
        for ancestor in file.ancestors().skip(1) {
            if !directories.contains(ancestor) {
                break;
            }
            let total = directory_bytes.entry(ancestor).or_default();
            *total = total.saturating_add(bytes);
        }
    }
    let files = plan.files.iter().map(|path| (path, RowKind::File));
    let removals = plan.removals.iter().map(|path| {
        let is_link = fs::symlink_metadata(path).is_ok_and(|found| found.file_type().is_symlink());
        let kind = if is_link {
            RowKind::Symlink
        } else {
            RowKind::Special
        };
        (path, kind)
    });
    let directories = plan
        .directories
        .iter()
        .map(|path| (path, RowKind::Directory));
    let unreadable = statistics
        .unreadable
        .iter()
        .map(|path| (path, RowKind::Unreadable));
    let mut rows: Vec<Row<'_>> = files
        .chain(removals)
        .chain(directories)
        .chain(unreadable)
        .map(|(path, kind)| {
            let bytes = match kind {
                RowKind::File => Some(plan.size(path)),
                RowKind::Directory => {
                    Some(directory_bytes.get(path.as_path()).copied().unwrap_or(0))
                }
                _ => None,
            };
            Row {
                path,
                depth: depth(plan, path),
                kind,
                bytes,
                linked: linked.contains(path.as_path()),
            }
        })
        .collect();
    rows.sort_by(|left, right| left.path.cmp(right.path));
    rows
}

/// How far below its outermost root a target lies.
fn depth(plan: &Plan, path: &Path) -> usize {
    plan.base_of(path).map_or(0, |base| {
        path.components()
            .count()
            .saturating_sub(base.components().count() + 1)
    })
}
