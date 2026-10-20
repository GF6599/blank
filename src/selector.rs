use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;

use ratatui::style::Color;

use crate::chrome::{
    CONTROLS_GAP, Column, LOVE, Line, MUTED, SUBTLE, TEXT, cells, clamp_tail, draw_controls,
    draw_mark, draw_panel, draw_text_panel, panel_rows, put, wrap,
};
use crate::guard::Guard;
use crate::plan::{counted, short_size};
use crate::preview::{Preview, Totals, preview};
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
const PREVIEW_TITLE: &str = "preview";
/// Below this column width the panes cannot sit side by side and both stay readable.
const SIDE_BY_SIDE_WIDTH: u16 = 64;
/// Below this height a stacked preview would leave the listing too few rows to browse.
const STACKED_HEIGHT: u16 = 10;
/// A hex line is the offset and two gaps of two cells, then four cells a byte, less the space
/// that the last byte of hex does not take.
const HEX_OVERHEAD: u16 = 4 + 2 + 2 - 1;
const HEX_CELLS_PER_BYTE: u16 = 4;
const HEX_MOST_BYTES: u16 = 16;

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

/// A read the selector asks the screen to run off its thread. Each carries the flag that a
/// newer request of its kind sets, so a walk nobody will read ends early.
pub enum Work {
    Preview {
        id: u64,
        path: PathBuf,
        guard: Arc<Guard>,
        stop: Arc<AtomicBool>,
    },
    Totals {
        id: u64,
        paths: Vec<PathBuf>,
        guard: Arc<Guard>,
        stop: Arc<AtomicBool>,
    },
}

/// What a piece of work found, or nothing when it was stopped.
pub enum Finished {
    Preview {
        id: u64,
        preview: Option<Preview>,
    },
    Totals {
        id: u64,
        totals: Option<Result<Totals, String>>,
    },
}

impl Work {
    pub fn run(&self) -> Finished {
        match self {
            Work::Preview {
                id,
                path,
                guard,
                stop,
            } => Finished::Preview {
                id: *id,
                preview: preview(path, guard, stop),
            },
            Work::Totals {
                id,
                paths,
                guard,
                stop,
            } => Finished::Totals {
                id: *id,
                totals: Totals::of_paths(paths, guard, stop),
            },
        }
    }
}

/// The totals of the marked set, as the header shows them.
enum Totalling {
    Nothing,
    Counting,
    Counted(Result<Totals, String>),
}

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
    /// The path the preview pane is for, and what it found, which is nothing while it reads.
    previewed: Option<PathBuf>,
    preview: Option<Preview>,
    preview_id: u64,
    preview_stop: Arc<AtomicBool>,
    marks_changed: bool,
    totals: Totalling,
    totals_id: u64,
    totals_stop: Arc<AtomicBool>,
}

impl Drop for Selector {
    /// The run that follows the selector needs the disk, and a walk for a pane nobody sees
    /// anymore would compete with it.
    fn drop(&mut self) {
        self.preview_stop.store(true, Ordering::Relaxed);
        self.totals_stop.store(true, Ordering::Relaxed);
    }
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
            previewed: None,
            preview: None,
            preview_id: 0,
            preview_stop: Arc::new(AtomicBool::new(false)),
            marks_changed: false,
            totals: Totalling::Nothing,
            totals_id: 0,
            totals_stop: Arc::new(AtomicBool::new(false)),
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
        self.marks_changed = true;
    }

    /// The reads the last key made necessary: a preview when the cursor reached another entry,
    /// and the totals when the marked set changed. Each stops the read of its kind before it.
    pub fn work(&mut self) -> Vec<Work> {
        let mut work = Vec::new();
        let current = self.selected().map(|entry| entry.path.clone());
        if !self.loading && current != self.previewed {
            self.preview_stop.store(true, Ordering::Relaxed);
            self.preview_stop = Arc::new(AtomicBool::new(false));
            self.preview_id += 1;
            self.preview = None;
            self.previewed.clone_from(&current);
            if let Some(path) = current {
                work.push(Work::Preview {
                    id: self.preview_id,
                    path,
                    guard: Arc::clone(&self.guard),
                    stop: Arc::clone(&self.preview_stop),
                });
            }
        }
        if self.marks_changed {
            self.marks_changed = false;
            self.totals_stop.store(true, Ordering::Relaxed);
            self.totals_stop = Arc::new(AtomicBool::new(false));
            self.totals_id += 1;
            if self.marked.is_empty() {
                self.totals = Totalling::Nothing;
            } else {
                self.totals = Totalling::Counting;
                work.push(Work::Totals {
                    id: self.totals_id,
                    paths: self.marked.iter().cloned().collect(),
                    guard: Arc::clone(&self.guard),
                    stop: Arc::clone(&self.totals_stop),
                });
            }
        }
        work
    }

    /// Takes what a piece of work found, unless newer work of its kind replaced it.
    pub fn finished(&mut self, finished: Finished) {
        match finished {
            Finished::Preview {
                id,
                preview: Some(found),
            } if id == self.preview_id => self.preview = Some(found),
            Finished::Totals {
                id,
                totals: Some(found),
            } if id == self.totals_id => self.totals = Totalling::Counted(found),
            _ => {}
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
        // A size cut to fit reads as a smaller number, so a row with no room for a whole
        // wording stays empty.
        if let Some((wordings, color)) = self.totals_wordings()
            && let Some(fits) = wordings
                .iter()
                .find(|wording| cells(wording) <= column.width)
        {
            put(
                buffer,
                column.x,
                status_row + 1,
                fits,
                Style::new().fg(color),
            );
        }
        let panes_top = status_row + 3;
        let panes = Rect::new(
            column.x,
            panes_top,
            column.width,
            area.bottom().saturating_sub(panes_top),
        );
        self.draw_panes(buffer, panes);
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

    /// The totals row under the status row in every length it comes in, longest first, or
    /// nothing while no entry is marked. A narrow screen drops the label and then the counts,
    /// and keeps the size, which says the most in the fewest cells.
    fn totals_wordings(&self) -> Option<(Vec<String>, Color)> {
        match &self.totals {
            Totalling::Nothing => None,
            Totalling::Counting => Some((
                vec!["counting the marked set".to_owned(), "counting".to_owned()],
                MUTED,
            )),
            Totalling::Counted(Err(refusal)) => {
                Some((vec![format!("{FAILED_MARK} {refusal}")], LOVE))
            }
            Totalling::Counted(Ok(totals)) => {
                let parts = totals_parts(totals).join(", ");
                let size = short_size(totals.bytes);
                Some((
                    vec![format!("marked: {parts}"), parts, format!("{size} marked")],
                    SUBTLE,
                ))
            }
        }
    }

    /// The listing and the preview side by side on a wide terminal, one above the other on a
    /// narrow one with the height for both, and the listing alone otherwise.
    fn draw_panes(&mut self, buffer: &mut Buffer, area: Rect) {
        if area.width >= SIDE_BY_SIDE_WIDTH {
            let list_width = area.width / 2;
            let list = Rect::new(area.x, area.y, list_width, area.height);
            let pane = Rect::new(
                area.x + list_width + 1,
                area.y,
                area.width - list_width - 1,
                area.height,
            );
            self.draw_listing(buffer, list);
            self.draw_preview(buffer, pane);
        } else if area.height >= STACKED_HEIGHT {
            let list_height = area.height - area.height / 2;
            let list = Rect::new(area.x, area.y, area.width, list_height);
            let pane = Rect::new(
                area.x,
                area.y + list_height,
                area.width,
                area.height - list_height,
            );
            self.draw_listing(buffer, list);
            self.draw_preview(buffer, pane);
        } else {
            self.draw_listing(buffer, area);
        }
    }

    fn draw_preview(&self, buffer: &mut Buffer, area: Rect) {
        // The frame's two rules and a cell of indent either side.
        let width = area.width.saturating_sub(4);
        let lines = self.preview_lines(width);
        draw_text_panel(buffer, area, PREVIEW_TITLE, &lines);
    }

    fn preview_lines(&self, width: u16) -> Vec<(String, Color)> {
        let failure = |message: String| {
            wrap(&format!("{FAILED_MARK} {message}"), width)
                .into_iter()
                .map(|line| (line, LOVE))
                .collect()
        };
        let Some(found) = &self.preview else {
            let reading = match self.selected() {
                None => return Vec::new(),
                Some(entry) if entry.is_dir => "counting",
                Some(_) => "reading",
            };
            return vec![(reading.to_owned(), MUTED)];
        };
        match found {
            Preview::Text(lines) if lines.is_empty() => vec![("empty file".to_owned(), MUTED)],
            Preview::Text(lines) => lines.iter().map(|line| (line.clone(), TEXT)).collect(),
            Preview::Binary(head) => hex_dump(head, width),
            Preview::Directory(totals) => {
                std::iter::once(("marking it destroys".to_owned(), MUTED))
                    // A count cut to fit reads as a smaller one, so a part that does not fit whole is
                    // left out.
                    .chain(
                        totals_parts(totals)
                            .into_iter()
                            .filter(|part| cells(part) <= width)
                            .map(|part| (part, TEXT)),
                    )
                    .collect()
            }
            Preview::Symlink(target) => std::iter::once(("symlink to".to_owned(), MUTED))
                .chain(wrap(target, width).into_iter().map(|line| (line, TEXT)))
                .collect(),
            Preview::Special(kind) => vec![(format!("{kind}, never opened"), TEXT)],
            Preview::Refused(refusal) => failure(refusal.clone()),
            Preview::Unreadable(error) => failure(format!("cannot read: {error}")),
        }
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

/// The kinds a run would destroy, those with none left out, then the size, then the directories
/// that would survive because nobody could list them.
fn totals_parts(totals: &Totals) -> Vec<String> {
    let mut parts: Vec<String> = [
        (totals.files, "file", "files"),
        (
            totals.removals,
            "symlink or special file",
            "symlinks and special files",
        ),
        (totals.directories, "directory", "directories"),
    ]
    .into_iter()
    .filter(|(count, _, _)| *count > 0)
    .map(|(count, one, many)| counted(count, one, many))
    .collect();
    parts.push(short_size(totals.bytes));
    if totals.unreadable > 0 {
        let many = "unreadable directories";
        parts.push(counted(totals.unreadable, "unreadable directory", many));
    }
    parts
}

/// The head of a binary file, as many bytes a line as the width holds, each line an offset, the
/// bytes in hex, and the printable ones as text.
fn hex_dump(head: &[u8], width: u16) -> Vec<(String, Color)> {
    let per_line =
        (width.saturating_sub(HEX_OVERHEAD) / HEX_CELLS_PER_BYTE).clamp(1, HEX_MOST_BYTES);
    head.chunks(usize::from(per_line))
        .enumerate()
        .map(|(index, chunk)| {
            let hex: Vec<String> = chunk.iter().map(|byte| format!("{byte:02x}")).collect();
            let text: String = chunk
                .iter()
                .map(|&byte| {
                    if byte.is_ascii_graphic() || byte == b' ' {
                        char::from(byte)
                    } else {
                        '.'
                    }
                })
                .collect();
            let offset = index * usize::from(per_line);
            let columns = usize::from(per_line) * 3 - 1;
            (
                format!("{offset:04x}  {:<columns$}  {text}", hex.join(" ")),
                TEXT,
            )
        })
        .collect()
}
