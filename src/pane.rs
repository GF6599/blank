use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use crate::chrome::{LOVE, MUTED, TEXT, cells, draw_text_panel, wrap};
use crate::guard::Guard;
use crate::plan::{counted, short_size};
use crate::preview::{Preview, Totals, preview};

const PREVIEW_TITLE: &str = "preview";
const FAILED_MARK: &str = "!";
/// A hex line is the offset and two gaps of two cells, then four cells a byte, less the space
/// that the last byte of hex does not take.
const HEX_OVERHEAD: u16 = 4 + 2 + 2 - 1;
const HEX_CELLS_PER_BYTE: u16 = 4;
const HEX_MOST_BYTES: u16 = 16;

/// The preview of the row under a cursor. It never reads the disk itself: it asks for a read
/// when the cursor reaches another path, and a newer read stops the walk of the one before it,
/// so a walk nobody will read ends at its next entry.
pub(crate) struct PreviewPane {
    guard: Arc<Guard>,
    /// The line that opens the totals of a directory, which says what they count on this
    /// screen.
    directory_heading: &'static str,
    /// The path the pane is for, and what it found, which is nothing while it reads.
    previewed: Option<PathBuf>,
    preview: Option<Preview>,
    id: u64,
    stop: Arc<AtomicBool>,
}

/// A preview the pane asks the screen to read off its thread.
pub struct PreviewRequest {
    id: u64,
    path: PathBuf,
    guard: Arc<Guard>,
    stop: Arc<AtomicBool>,
}

/// What a preview read found, or nothing when a newer read stopped it.
pub struct PreviewResult {
    id: u64,
    preview: Option<Preview>,
}

impl PreviewRequest {
    pub fn run(&self) -> PreviewResult {
        PreviewResult {
            id: self.id,
            preview: preview(&self.path, &self.guard, &self.stop),
        }
    }
}

impl Drop for PreviewPane {
    /// The screen that follows needs the disk, and a walk for a pane nobody sees anymore would
    /// compete with it.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl PreviewPane {
    pub(crate) fn new(guard: Arc<Guard>, directory_heading: &'static str) -> PreviewPane {
        PreviewPane {
            guard,
            directory_heading,
            previewed: None,
            preview: None,
            id: 0,
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Asks for a read when the cursor reached another path, and stops the read before it.
    pub(crate) fn follow(&mut self, path: Option<&Path>) -> Option<PreviewRequest> {
        if path == self.previewed.as_deref() {
            return None;
        }
        self.stop.store(true, Ordering::Relaxed);
        self.stop = Arc::new(AtomicBool::new(false));
        self.id += 1;
        self.preview = None;
        self.previewed = path.map(Path::to_path_buf);
        let path = self.previewed.clone()?;
        Some(PreviewRequest {
            id: self.id,
            path,
            guard: Arc::clone(&self.guard),
            stop: Arc::clone(&self.stop),
        })
    }

    /// Takes what a read found, unless a newer read replaced it.
    pub(crate) fn finished(&mut self, result: PreviewResult) {
        if let PreviewResult {
            id,
            preview: Some(found),
        } = result
            && id == self.id
        {
            self.preview = Some(found);
        }
    }

    /// `pending` says what the pane is doing while the read runs.
    pub(crate) fn draw(&self, buffer: &mut Buffer, area: Rect, pending: &str) {
        // The frame's two rules and a cell of indent either side.
        let width = area.width.saturating_sub(4);
        let lines = self.lines(width, pending);
        draw_text_panel(buffer, area, PREVIEW_TITLE, &lines);
    }

    fn lines(&self, width: u16, pending: &str) -> Vec<(String, Color)> {
        let failure = |message: String| {
            wrap(&format!("{FAILED_MARK} {message}"), width)
                .into_iter()
                .map(|line| (line, LOVE))
                .collect()
        };
        if self.previewed.is_none() {
            return Vec::new();
        }
        let Some(found) = &self.preview else {
            return vec![(pending.to_owned(), MUTED)];
        };
        match found {
            Preview::Text(lines) if lines.is_empty() => vec![("empty file".to_owned(), MUTED)],
            Preview::Text(lines) => lines.iter().map(|line| (line.clone(), TEXT)).collect(),
            Preview::Binary(head) => hex_dump(head, width),
            Preview::Directory(totals) => {
                std::iter::once((self.directory_heading.to_owned(), MUTED))
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
}

/// The kinds a run would destroy, those with none left out, then the size, then the directories
/// that would survive because nobody could list them.
pub(crate) fn totals_parts(totals: &Totals) -> Vec<String> {
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
