use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::widgets::StatefulWidget;
use ratatui_image::StatefulImage;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;

use crate::chrome::{
    LOVE, MUTED, TEXT, cells, draw_text_panel, text_panel_area, text_panel_width, wrap,
};
use crate::guard::Guard;
use crate::plan::{counted, short_size};
use crate::preview::{Picture, Preview, Totals, preview};
use crate::render::Renderer;

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
    renderer: Arc<Renderer>,
    picker: Picker,
    /// The image of the preview, encoded for the terminal at the size it was last drawn.
    image: Option<StatefulProtocol>,
}

/// A preview the pane asks the screen to read off its thread.
pub struct PreviewRequest {
    id: u64,
    path: PathBuf,
    guard: Arc<Guard>,
    renderer: Arc<Renderer>,
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
            preview: preview(&self.path, &self.guard, &self.renderer, &self.stop),
        }
    }
}

impl Drop for PreviewPane {
    /// The screen that follows needs the disk, and a walk for a pane nobody sees anymore would
    /// compete with it. A tool still drawing is killed, and its renders are removed.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.renderer.close();
    }
}

impl PreviewPane {
    pub(crate) fn new(
        guard: Arc<Guard>,
        directory_heading: &'static str,
        picker: Picker,
    ) -> PreviewPane {
        PreviewPane {
            guard,
            directory_heading,
            previewed: None,
            preview: None,
            id: 0,
            stop: Arc::new(AtomicBool::new(false)),
            renderer: Arc::new(Renderer::for_host()),
            picker,
            image: None,
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
        self.image = None;
        self.previewed = path.map(Path::to_path_buf);
        let path = self.previewed.clone()?;
        Some(PreviewRequest {
            id: self.id,
            path,
            guard: Arc::clone(&self.guard),
            renderer: Arc::clone(&self.renderer),
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
            self.image = match &found {
                Preview::Image(Picture { image, .. }) | Preview::Rendered { image, .. } => {
                    Some(self.picker.new_resize_protocol(image.clone()))
                }
                _ => None,
            };
            self.preview = Some(found);
        }
    }

    /// `pending` says what the pane is doing while the read runs. An image goes under its
    /// caption, in the rows the lines leave.
    pub(crate) fn draw(&mut self, buffer: &mut Buffer, area: Rect, pending: &str) {
        let lines = self.lines(text_panel_width(area), pending);
        draw_text_panel(buffer, area, PREVIEW_TITLE, &lines);
        if let Some(image) = &mut self.image {
            let text = text_panel_area(area);
            let used = u16::try_from(lines.len())
                .unwrap_or(u16::MAX)
                .min(text.height);
            let below = Rect::new(text.x, text.y + used, text.width, text.height - used);
            StatefulImage::default().render(below, buffer, image);
        }
    }

    fn lines(&self, width: u16, pending: &str) -> Vec<(String, Color)> {
        if self.previewed.is_none() {
            return Vec::new();
        }
        match &self.preview {
            Some(found) => self.found_lines(found, width),
            None => vec![(pending.to_owned(), MUTED)],
        }
    }

    fn found_lines(&self, found: &Preview, width: u16) -> Vec<(String, Color)> {
        let failure = |message: String| {
            wrap(&format!("{FAILED_MARK} {message}"), width)
                .into_iter()
                .map(|line| (line, LOVE))
                .collect()
        };
        match found {
            Preview::Text(lines) if lines.is_empty() => vec![("empty file".to_owned(), MUTED)],
            Preview::Text(lines) => lines.iter().map(|line| (line.clone(), TEXT)).collect(),
            Preview::Binary(head) => hex_dump(head, width),
            Preview::Image(picture) => vec![(caption(picture), MUTED)],
            Preview::Rendered { kind, .. } => vec![(kind.caption().to_owned(), MUTED)],
            Preview::Undecoded { reason, contents } => wrap(reason, width)
                .into_iter()
                .map(|line| (line, MUTED))
                .chain(self.found_lines(contents, width))
                .collect(),
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

/// The format and the size the file holds, because the pane shows the image shrunk.
fn caption(picture: &Picture) -> String {
    let format = picture.format.extensions_str().first().map_or_else(
        || format!("{:?}", picture.format),
        |name| name.to_uppercase(),
    );
    format!("{format}, {} × {}", picture.width, picture.height)
}

/// The kinds a run would destroy, those with none left out.
pub(crate) fn kind_parts(totals: &Totals) -> Vec<String> {
    [
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
    .collect()
}

/// The kinds a run would destroy, then the size, then the directories that would survive
/// because nobody could list them.
pub(crate) fn totals_parts(totals: &Totals) -> Vec<String> {
    let mut parts = kind_parts(totals);
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
