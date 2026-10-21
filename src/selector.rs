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
use ratatui_image::picker::Picker;

use crate::chrome::{
    CONTROLS_GAP, Column, ELLIPSIS, LOVE, Line, MUTED, SUBTLE, TEXT, cells, clamp_tail,
    draw_controls, draw_mark, draw_panel, draw_text_panel, panel_rows, put, split_panes,
    split_three, text_panel_width, wrap,
};
use crate::engine::Options;
use crate::guard::Guard;
use crate::pane::{PreviewPane, PreviewRequest, PreviewResult, kind_parts, totals_parts};
use crate::plan::{cover_bytes, linked_sentence, short_size, unreadable_sentence};
use crate::preview::Totals;
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

const MARKED_TITLE: &str = "marked";
const NOTHING_MARKED: &str = "nothing marked";
const COUNTING: &str = "counting";
const LARGEST_HEADING: &str = "largest";
const LARGEST_GAP: u16 = 2;

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
    Preview(PreviewRequest),
    Totals {
        id: u64,
        paths: Vec<PathBuf>,
        guard: Arc<Guard>,
        stop: Arc<AtomicBool>,
    },
}

/// What a piece of work found, or nothing when it was stopped.
pub enum Finished {
    Preview(PreviewResult),
    Totals {
        id: u64,
        totals: Option<Result<Totals, String>>,
    },
}

impl Work {
    pub fn run(&self) -> Finished {
        match self {
            Work::Preview(request) => Finished::Preview(request.run()),
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
    pane: PreviewPane,
    marks_changed: bool,
    totals: Totalling,
    totals_id: u64,
    totals_stop: Arc<AtomicBool>,
    /// The options of the run the marked set goes to, which decide what its covers write.
    options: Options,
}

impl Drop for Selector {
    /// The run that follows the selector needs the disk, and a walk for a pane nobody sees
    /// anymore would compete with it.
    fn drop(&mut self) {
        self.totals_stop.store(true, Ordering::Relaxed);
    }
}

impl Selector {
    /// A selector on `dir`, and the read of its first listing.
    pub fn open(
        dir: &Path,
        guard: Arc<Guard>,
        options: Options,
        picker: Picker,
    ) -> (Selector, LoadRequest) {
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
            pane: PreviewPane::new(Arc::clone(&guard), "marking it destroys", picker),
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
            marks_changed: false,
            totals: Totalling::Nothing,
            totals_id: 0,
            totals_stop: Arc::new(AtomicBool::new(false)),
            options,
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
        if !self.loading {
            let current = self.selected().map(|entry| entry.path.clone());
            if let Some(request) = self.pane.follow(current.as_deref()) {
                work.push(Work::Preview(request));
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
            Finished::Preview(result) => self.pane.finished(result),
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
        let panes_top = status_row + 3;
        let panes = Rect::new(
            column.x,
            panes_top,
            column.width,
            area.bottom().saturating_sub(panes_top),
        );
        if let Some([list, preview, marked]) = split_three(panes) {
            self.draw_listing(buffer, list);
            self.draw_preview(buffer, preview);
            let width = text_panel_width(marked);
            draw_text_panel(buffer, marked, MARKED_TITLE, &self.statistics_lines(width));
        } else {
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
            self.draw_panes(buffer, panes);
        }
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

    /// Every line fits `width`, because the panel cuts a longer one, and a size cut short reads
    /// as a smaller one.
    fn statistics_lines(&self, width: u16) -> Vec<(String, Color)> {
        let fitted = |text: &str, color: Color| -> Vec<(String, Color)> {
            wrap(text, width)
                .into_iter()
                .map(|line| (line, color))
                .collect()
        };
        let totals = match &self.totals {
            Totalling::Nothing => return fitted(NOTHING_MARKED, MUTED),
            Totalling::Counting => return fitted(COUNTING, MUTED),
            Totalling::Counted(Err(refusal)) => {
                return fitted(&format!("{FAILED_MARK} {refusal}"), LOVE);
            }
            Totalling::Counted(Ok(totals)) => totals,
        };
        let passes = self.options.passes;
        let covers = passes.saturating_add(1);
        let noun = if covers == 1 { "pass" } else { "passes" };
        let mut lines: Vec<(String, Color)> = kind_parts(totals)
            .into_iter()
            .flat_map(|part| fitted(&part, TEXT))
            .collect();
        lines.extend(fitted(&format!("holds {}", short_size(totals.bytes)), TEXT));
        let written = short_size(cover_bytes(totals.bytes, passes));
        lines.extend(fitted(
            &format!("covers write {written} in {covers} {noun}"),
            TEXT,
        ));
        if !totals.largest.is_empty() {
            lines.push((String::new(), TEXT));
            lines.push((LARGEST_HEADING.to_owned(), MUTED));
            let sizes: Vec<String> = totals
                .largest
                .iter()
                .map(|(_, size)| short_size(*size))
                .collect();
            let size_width = sizes.iter().map(|size| cells(size)).max().unwrap_or(0);
            for ((path, _), size) in totals.largest.iter().zip(&sizes) {
                let room = width.saturating_sub(size_width + LARGEST_GAP);
                let shown = path.strip_prefix(&self.dir).unwrap_or(path);
                let name = clamp_tail(&escape_path(shown), room, ELLIPSIS);
                let gap = usize::from(size_width + LARGEST_GAP) - cells(size) as usize;
                lines.push((format!("{size}{}{name}", " ".repeat(gap)), SUBTLE));
            }
        }
        let leaves = [
            unreadable_sentence(totals.unreadable).map(|sentence| (sentence, LOVE)),
            linked_sentence(totals.linked, self.options.allow_hard_links).map(|sentence| {
                let color = if self.options.allow_hard_links {
                    SUBTLE
                } else {
                    LOVE
                };
                (sentence, color)
            }),
        ];
        for (sentence, color) in leaves.into_iter().flatten() {
            lines.push((String::new(), TEXT));
            lines.extend(fitted(&sentence, color));
        }
        lines
    }

    /// The listing and the preview side by side on a wide terminal, one above the other on a
    /// narrow one with the height for both, and the listing alone otherwise.
    fn draw_panes(&mut self, buffer: &mut Buffer, area: Rect) {
        let (list, pane) = split_panes(area);
        self.draw_listing(buffer, list);
        if let Some(pane) = pane {
            self.draw_preview(buffer, pane);
        }
    }

    fn draw_preview(&mut self, buffer: &mut Buffer, area: Rect) {
        let pending = match self.selected() {
            Some(entry) if entry.is_dir => "counting",
            _ => "reading",
        };
        self.pane.draw(buffer, area, pending);
    }

    fn draw_listing(&mut self, buffer: &mut Buffer, area: Rect) {
        self.panel_rows = panel_rows(area);
        // A taller screen than the last one shows again the rows above, rather than empty rows
        // below.
        self.first = self
            .first
            .min(self.shown().len().saturating_sub(self.panel_rows));
        if self.cursor < self.first {
            self.first = self.cursor;
        } else if self.cursor >= self.first + self.panel_rows {
            self.first = self.cursor + 1 - self.panel_rows;
        }
        // The frame spends two corners, the lead rune, a cell either side of the title, and a
        // cell of fill.
        let title = clamp_tail(
            &escape_path(&self.dir),
            area.width.saturating_sub(6),
            ELLIPSIS,
        );
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
                    detail: None,
                    current: index == self.cursor,
                }
            });
        draw_panel(buffer, area, &title, lines, shown.len(), self.first);
    }
}
