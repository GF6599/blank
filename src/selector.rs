use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;

use crate::chrome::{
    CONTROLS_GAP, Column, LOVE, Line, MUTED, SUBTLE, TEXT, cells, clamp_tail, draw_controls,
    draw_mark, draw_panel, panel_rows, put,
};
use crate::guard::Guard;
use crate::terminal::escape_path;

/// Marking a directory destroys the whole tree under it, and marking a symlink destroys the
/// link and never what it points at. Neither is what the bare name suggests, so both say which
/// they are.
const DIRECTORY_SUFFIX: &str = "/";
const SYMLINK_SUFFIX: &str = "@";
/// ASCII, so they read on any terminal and in any font.
const MARKED_MARK: &str = "*";
const REFUSED_MARK: &str = "-";
const FAILED_MARK: &str = "!";

const CONTROLS: [&str; 3] = [
    "[space] mark  [enter] done  [q] quit",
    "[enter] done  [q] quit",
    "[q] quit",
];
const FILTER_PROMPT: &str = "filter: ";

pub enum Pick {
    None,
    /// Read a directory off the screen's thread, and hand the result to `Selector::loaded`.
    Load(LoadRequest),
    /// Hand the marked set to the run.
    Done,
    Quit,
}

/// A directory to read. It is the only part of the selector that touches the disk.
pub struct LoadRequest {
    dir: PathBuf,
    /// Set on the way up, the only way the selector can reach a protected directory. A
    /// directory that holds one is refused itself, so going down never needs the check.
    check: bool,
    guard: Arc<Guard>,
}

pub struct Loaded(Result<Listing, String>);

struct Listing {
    dir: PathBuf,
    entries: Vec<Entry>,
    error: Option<String>,
}

struct Entry {
    path: PathBuf,
    label: String,
    /// False for a symlink to a directory, which the selector never enters.
    is_dir: bool,
    /// Why the run would refuse this path, decided when the listing is read so that a draw
    /// never touches the disk.
    refusal: Option<String>,
}

impl LoadRequest {
    pub fn run(&self) -> Loaded {
        if self.check
            && let Err(refusal) = self.guard.check(&self.dir)
        {
            return Loaded(Err(refusal.to_string()));
        }
        let (entries, error) = match read_entries(&self.dir, &self.guard) {
            Ok(entries) => (entries, None),
            // The frame title names the directory, so the status row keeps its room for why.
            Err(error) => (Vec::new(), Some(format!("read directory: {error}"))),
        };
        Loaded(Ok(Listing {
            dir: self.dir.clone(),
            entries,
            error,
        }))
    }
}

fn read_entries(dir: &Path, guard: &Guard) -> std::io::Result<Vec<Entry>> {
    let mut found = fs::read_dir(dir)?.collect::<std::io::Result<Vec<_>>>()?;
    found.sort_by_key(|entry| entry.file_name());
    Ok(found
        .into_iter()
        .map(|entry| {
            let path = entry.path();
            let kind = entry.file_type().ok();
            let is_link = kind.is_some_and(|kind| kind.is_symlink());
            let is_dir = kind.is_some_and(|kind| kind.is_dir());
            let suffix = match (is_link, is_dir) {
                (true, _) => SYMLINK_SUFFIX,
                (false, true) => DIRECTORY_SUFFIX,
                _ => "",
            };
            Entry {
                label: format!("{}{suffix}", escape_path(Path::new(&entry.file_name()))),
                is_dir,
                refusal: guard.check(&path).err().map(|refusal| refusal.to_string()),
                path,
            }
        })
        .collect())
}

/// The state of the selector that `blank` opens with no arguments. It marks paths across
/// directories and hands the marked set to the run, which checks it again like any command
/// line.
pub struct Selector {
    dir: PathBuf,
    entries: Vec<Entry>,
    marked: BTreeSet<PathBuf>,
    guard: Arc<Guard>,
    filter: String,
    filtering: bool,
    /// The position of the cursor among the entries the filter shows.
    cursor: usize,
    first: usize,
    panel_rows: usize,
    loading: bool,
    read_error: Option<String>,
    /// The answer to the key just pressed, so it lives until the next key rather than staying
    /// on screen over an unrelated row.
    refusal: Option<String>,
    confirmed: bool,
}

impl Selector {
    /// A selector on `dir`, and the read of its first listing.
    pub fn open(dir: &Path, guard: Arc<Guard>) -> (Selector, LoadRequest) {
        // A relative name of one part has an empty parent, so a selector that kept it could
        // never leave the directory it started in.
        let dir = fs::canonicalize(dir)
            .or_else(|_| std::path::absolute(dir))
            .unwrap_or_else(|_| dir.to_path_buf());
        let load = LoadRequest {
            dir: dir.clone(),
            check: false,
            guard: Arc::clone(&guard),
        };
        let selector = Selector {
            dir,
            entries: Vec::new(),
            marked: BTreeSet::new(),
            guard,
            filter: String::new(),
            filtering: false,
            cursor: 0,
            first: 0,
            panel_rows: 1,
            loading: true,
            read_error: None,
            refusal: None,
            confirmed: false,
        };
        (selector, load)
    }

    pub fn directory(&self) -> &Path {
        &self.dir
    }

    /// The marked paths in a stable order, and nothing unless the operator chose done.
    pub fn targets(&self) -> Vec<PathBuf> {
        if !self.confirmed {
            return Vec::new();
        }
        self.marked.iter().cloned().collect()
    }

    pub fn loaded(&mut self, loaded: Loaded) {
        self.loading = false;
        match loaded.0 {
            Ok(listing) => {
                self.dir = listing.dir;
                self.entries = listing.entries;
                self.read_error = listing.error;
                self.filter.clear();
                self.filtering = false;
                self.cursor = 0;
                self.first = 0;
            }
            Err(refusal) => self.refusal = Some(refusal),
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> Pick {
        self.refusal = None;
        // Ahead of the filter, because a screen that routes ctrl+c into a text input cannot be
        // left at all.
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Pick::Quit;
        }
        // A filename holds spaces and every letter the selector binds, so while the filter is
        // typed every other key is text.
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
                _ => return Pick::None,
            }
            self.cursor = 0;
            return Pick::None;
        }
        let page = self.panel_rows.max(1);
        match key.code {
            KeyCode::Char('q') => return Pick::Quit,
            // Escape backs out of the filter before it backs out of the selector.
            KeyCode::Esc if self.filter.is_empty() => return Pick::Quit,
            KeyCode::Esc => {
                self.filter.clear();
                self.cursor = 0;
            }
            KeyCode::Enter => {
                self.confirmed = true;
                return Pick::Done;
            }
            KeyCode::Char(' ') => self.toggle(),
            KeyCode::Char('/') => self.filtering = true,
            KeyCode::Char('l') | KeyCode::Right => return self.descend(),
            KeyCode::Char('h') | KeyCode::Left | KeyCode::Backspace => return self.ascend(),
            KeyCode::Char('j') | KeyCode::Down => self.move_cursor(1, 0),
            KeyCode::Char('k') | KeyCode::Up => self.move_cursor(0, 1),
            KeyCode::PageDown => self.move_cursor(page, 0),
            KeyCode::PageUp => self.move_cursor(0, page),
            KeyCode::Home => self.move_cursor(0, usize::MAX),
            KeyCode::End => self.move_cursor(usize::MAX, 0),
            _ => {}
        }
        Pick::None
    }

    fn shown(&self) -> Vec<&Entry> {
        let filter = self.filter.to_lowercase();
        self.entries
            .iter()
            .filter(|entry| filter.is_empty() || entry.label.to_lowercase().contains(&filter))
            .collect()
    }

    fn selected(&self) -> Option<&Entry> {
        self.shown().get(self.cursor).copied()
    }

    fn move_cursor(&mut self, down: usize, up: usize) {
        let last = self.shown().len().saturating_sub(1);
        self.cursor = self
            .cursor
            .saturating_add(down)
            .saturating_sub(up)
            .min(last);
    }

    /// The guard refuses a whole command line for one protected path, so a protected path
    /// marked here would cancel every ordinary target beside it after the operator finished
    /// choosing them.
    fn toggle(&mut self) {
        let Some(entry) = self.selected() else {
            return;
        };
        if let Some(refusal) = &entry.refusal {
            self.refusal = Some(refusal.clone());
            return;
        }
        let path = entry.path.clone();
        if !self.marked.remove(&path) {
            self.marked.insert(path);
        }
    }

    fn descend(&mut self) -> Pick {
        if self.loading {
            return Pick::None;
        }
        let Some(entry) = self.selected().filter(|entry| entry.is_dir) else {
            return Pick::None;
        };
        if let Some(refusal) = &entry.refusal {
            self.refusal = Some(refusal.clone());
            return Pick::None;
        }
        let dir = entry.path.clone();
        self.load(dir, false)
    }

    fn ascend(&mut self) -> Pick {
        if self.loading {
            return Pick::None;
        }
        let Some(parent) = self.dir.parent().map(Path::to_path_buf) else {
            return Pick::None;
        };
        self.load(parent, true)
    }

    fn load(&mut self, dir: PathBuf, check: bool) -> Pick {
        self.loading = true;
        Pick::Load(LoadRequest {
            dir,
            check,
            guard: Arc::clone(&self.guard),
        })
    }

    /// Every block shares one content column, the way the run screen builds its own.
    pub fn draw(&mut self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let column = Column::of(area);
        let buffer = frame.buffer_mut();
        let mark = draw_mark(buffer, column, area.y + 1, area.height);
        let status_row = area.y + 1 + mark + 1;
        let cursor = self.draw_status(buffer, column, status_row);
        let panel_top = status_row + 2;
        let panel = Rect::new(
            column.x,
            panel_top,
            column.width,
            area.bottom().saturating_sub(panel_top),
        );
        self.draw_listing(buffer, panel);
        if let Some(cursor) = cursor {
            frame.set_cursor_position(cursor);
        }
    }

    fn status_text(&self) -> (bool, String) {
        let failure = |message: &str| (true, message.to_owned());
        if let Some(refusal) = &self.refusal {
            return failure(refusal);
        }
        if self.loading {
            return (false, "loading".to_owned());
        }
        if self.filtering {
            return (false, format!("{FILTER_PROMPT}{}", self.filter));
        }
        if let Some(error) = &self.read_error {
            return failure(error);
        }
        let shown = self.shown().len();
        let position = if shown > 0 { self.cursor + 1 } else { 0 };
        let marked = self.marked.len();
        // An applied filter looks exactly like a small directory, so the count it hid is named.
        if shown != self.entries.len() {
            let held = self.entries.len();
            return (
                false,
                format!("{position}/{shown} of {held}  {marked} marked"),
            );
        }
        (false, format!("{position}/{shown}  {marked} marked"))
    }

    /// Returns where the terminal cursor belongs while the filter is typed.
    fn draw_status(&self, buffer: &mut Buffer, column: Column, y: u16) -> Option<Position> {
        let (failed, text) = self.status_text();
        let text = if failed {
            format!("{FAILED_MARK} {text}")
        } else {
            text
        };
        let width = column.width;
        let controls = CONTROLS
            .into_iter()
            .find(|controls| width >= cells(&text) + CONTROLS_GAP + cells(controls));
        // The tail, because the status that cannot fit is a failure, and its reason ends it.
        let shown = match controls {
            Some(_) => text,
            None => clamp_tail(&text, width, ""),
        };
        let style = Style::new().fg(SUBTLE);
        let end = put(buffer, column.x, y, &shown, style).0;
        if failed && shown.starts_with(FAILED_MARK) {
            put(buffer, column.x, y, FAILED_MARK, Style::new().fg(LOVE));
        }
        if self.filtering {
            put(buffer, column.x, y, FILTER_PROMPT, Style::new().fg(MUTED));
        }
        if let Some(controls) = controls {
            draw_controls(buffer, column, y, controls);
        }
        self.filtering.then_some(Position::new(end, y))
    }

    fn draw_listing(&mut self, buffer: &mut Buffer, area: Rect) {
        self.panel_rows = panel_rows(area);
        if self.cursor < self.first {
            self.first = self.cursor;
        } else if self.cursor >= self.first + self.panel_rows {
            self.first = self.cursor + 1 - self.panel_rows;
        }
        // The frame spends two corners, the lead rune, a cell either side of the title, and a
        // cell of fill.
        let title = clamp_tail(&escape_path(&self.dir), area.width.saturating_sub(6), "...");
        let shown = self.shown();
        let lines = shown
            .iter()
            .enumerate()
            .skip(self.first)
            .map(|(index, entry)| {
                let (state, state_color) = if entry.refusal.is_some() {
                    (REFUSED_MARK, MUTED)
                } else if self.marked.contains(&entry.path) {
                    // A marked entry is one the run will destroy, so it carries the colour of
                    // the survivor mark rather than the accent, which says something else.
                    (MARKED_MARK, LOVE)
                } else {
                    (" ", TEXT)
                };
                Line {
                    state,
                    state_color,
                    label: &entry.label,
                    current: index == self.cursor,
                }
            });
        draw_panel(buffer, area, &title, lines, shown.len(), self.first);
    }
}
