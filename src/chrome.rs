use std::ops::Range;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::symbols::border;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(crate) const LOVE: Color = Color::Rgb(0xeb, 0x6f, 0x92);
pub(crate) const GOLD: Color = Color::Rgb(0xf6, 0xc1, 0x77);
pub(crate) const PINE: Color = Color::Rgb(0x31, 0x74, 0x8f);
pub(crate) const FOAM: Color = Color::Rgb(0x9c, 0xcf, 0xd8);
pub(crate) const IRIS: Color = Color::Rgb(0xc4, 0xa7, 0xe7);
pub(crate) const TEXT: Color = Color::Rgb(0xe0, 0xde, 0xf4);
pub(crate) const SUBTLE: Color = Color::Rgb(0x90, 0x8c, 0xaa);
pub(crate) const MUTED: Color = Color::Rgb(0x6e, 0x6a, 0x86);
pub(crate) const HIGHLIGHT_MED: Color = Color::Rgb(0x40, 0x3d, 0x52);

/// The one corner lever. Every rule a screen draws reads its runes from here.
const BORDER: border::Set = border::PLAIN;

/// The marker is chrome saying "this one", and the label it marks is the item itself.
pub(crate) const MARKER: Color = GOLD;
pub(crate) const MARKED: Color = IRIS;

const PADDING: u16 = 2;
const MAX_WIDTH: u16 = 80;
/// Every wording the chrome can show survives this width, and below it nothing is left to cut
/// that still says which state a screen is in.
const MIN_CONTENT_WIDTH: u16 = 8;

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

pub(crate) const CONTROLS_GAP: u16 = 2;

/// "[ ", the state cell, " ", and " ]".
const MARKER_CELLS: u16 = 6;
/// A cell between the left rule and the bracket, which would otherwise weld to it.
const ROW_INDENT: u16 = 1;
/// Reserved whether or not the panel scrolls, so no label is cut again at the moment the
/// listing outgrows the panel.
const SCROLLBAR_CELLS: u16 = 2;
const SCROLL_THUMB: &str = "█";
const ELLIPSIS: &str = "...";

/// The column every block of a screen shares, so they line up on both edges.
#[derive(Clone, Copy)]
pub(crate) struct Column {
    pub(crate) x: u16,
    pub(crate) width: u16,
}

impl Column {
    pub(crate) fn of(area: Rect) -> Column {
        let width = area
            .width
            .saturating_sub(PADDING * 2)
            .clamp(MIN_CONTENT_WIDTH, MAX_WIDTH)
            .min(area.width.saturating_sub(PADDING));
        Column {
            x: area.x + PADDING.min(area.width),
            width,
        }
    }
}

/// Draws the mark from row `y`, and returns how many rows it took.
pub(crate) fn draw_mark(buffer: &mut Buffer, column: Column, y: u16, height: u16) -> u16 {
    let style = Style::new().fg(MARKED);
    let width = usize::from(column.width);
    if column.width < MARK_COLUMNS || height < MARK_MIN_HEIGHT {
        put_within(buffer, column.x, y, WORDMARK, width, style);
        return 1;
    }
    for (offset, line) in (0..).zip(MARK_ART) {
        put_within(buffer, column.x, y + offset, line, width, style);
    }
    MARK_ART.len() as u16
}

pub(crate) fn draw_controls(buffer: &mut Buffer, column: Column, y: u16, controls: &str) {
    let first = column.x + column.width - cells(controls);
    for (x, character) in (first..).zip(controls.chars()) {
        let color = if matches!(character, '[' | ']') {
            MARKER
        } else {
            MUTED
        };
        put(buffer, x, y, character.to_string(), Style::new().fg(color));
    }
}

pub(crate) struct Line<'a> {
    pub(crate) state: &'a str,
    pub(crate) state_color: Color,
    pub(crate) label: &'a str,
    pub(crate) current: bool,
}

pub(crate) fn panel_rows(area: Rect) -> usize {
    if area.height < 3 || area.width < 2 {
        1
    } else {
        usize::from(area.height - 2)
    }
}

/// Draws a frame with `title` cut into its top rule, and `lines` inside it from the top. The
/// thumb shows where the rows in view sit among `total`, the first of them at `first`.
pub(crate) fn draw_panel<'a>(
    buffer: &mut Buffer,
    area: Rect,
    title: &str,
    lines: impl IntoIterator<Item = Line<'a>>,
    total: usize,
    first: usize,
) {
    let Some(inner) = draw_frame(buffer, area, title) else {
        return;
    };
    let row_width = inner.width.saturating_sub(SCROLLBAR_CELLS + ROW_INDENT);
    let thumb = scrollbar(usize::from(inner.height), total, first);
    let mut lines = lines.into_iter();
    for offset in 0..inner.height {
        let y = inner.y + offset;
        if let Some(line) = lines.next() {
            draw_line(buffer, inner.x + ROW_INDENT, y, row_width, &line);
        }
        if thumb.contains(&usize::from(offset)) && inner.width >= SCROLLBAR_CELLS {
            let x = inner.right() - 1;
            put(buffer, x, y, SCROLL_THUMB, Style::new().fg(MUTED));
        }
    }
}

/// Draws a frame with `title` cut into its top rule, and `lines` inside it from the top, each
/// cut to the width of the frame.
pub(crate) fn draw_text_panel(
    buffer: &mut Buffer,
    area: Rect,
    title: &str,
    lines: &[(String, Color)],
) {
    let Some(inner) = draw_frame(buffer, area, title) else {
        return;
    };
    let width = inner.width.saturating_sub(ROW_INDENT * 2);
    for (offset, (text, color)) in (0..inner.height).zip(lines) {
        let shown = clamp_head(text, width);
        put(
            buffer,
            inner.x + ROW_INDENT,
            inner.y + offset,
            shown,
            Style::new().fg(*color),
        );
    }
}

/// Draws the rules of a frame, and returns the area inside them, or nothing when the frame has
/// no room for a row.
fn draw_frame(buffer: &mut Buffer, area: Rect, title: &str) -> Option<Rect> {
    if area.height < 3 || area.width < 2 {
        return None;
    }
    let border_style = Style::new().fg(HIGHLIGHT_MED);
    put(
        buffer,
        area.x,
        area.y,
        frame_top(area.width, title),
        border_style,
    );
    let inner = Rect::new(area.x + 1, area.y + 1, area.width - 2, area.height - 2);
    for y in inner.y..inner.bottom() {
        put(buffer, area.x, y, BORDER.vertical_left, border_style);
        put(
            buffer,
            area.right() - 1,
            y,
            BORDER.vertical_right,
            border_style,
        );
    }
    let bottom = format!(
        "{}{}{}",
        BORDER.bottom_left,
        BORDER.horizontal_bottom.repeat(usize::from(inner.width)),
        BORDER.bottom_right
    );
    put(buffer, area.x, area.bottom() - 1, bottom, border_style);
    Some(inner)
}

/// The bracket cells and the state cell are drawn on every row and left blank when they have
/// nothing to say, so a label never shifts as the marker moves or a state changes.
fn draw_line(buffer: &mut Buffer, x: u16, y: u16, width: u16, line: &Line<'_>) {
    let state_style = Style::new().fg(line.state_color);
    if width < MARKER_CELLS + 1 {
        put_within(buffer, x, y, line.state, usize::from(width), state_style);
        return;
    }
    let inner = width - MARKER_CELLS;
    let (lead, tail, label_color) = if line.current {
        ("[", "]", MARKED)
    } else {
        (" ", " ", TEXT)
    };
    let label = clamp_tail(line.label, inner, ELLIPSIS);
    put(buffer, x, y, lead, Style::new().fg(MARKER));
    put(buffer, x + 2, y, line.state, state_style);
    put(buffer, x + 4, y, &label, Style::new().fg(label_color));
    put(buffer, x + width - 1, y, tail, Style::new().fg(MARKER));
}

/// The top rule with the title cut in. The gap after the title is budgeted before the title
/// is admitted, so a title that only just fits cannot weld itself to the corner.
pub(crate) fn frame_top(width: u16, title: &str) -> String {
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
/// scroll, so the thumb reaches the last row when the last row is in view.
fn scrollbar(height: usize, total: usize, first: usize) -> Range<usize> {
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
pub(crate) fn clamp_tail(text: &str, width: u16, prefix: &str) -> String {
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

/// The start of a text, because a line of a file reads from its beginning.
pub(crate) fn clamp_head(text: &str, width: u16) -> String {
    let mut used = 0;
    text.chars()
        .take_while(|character| {
            used += character.width().unwrap_or(0);
            used <= usize::from(width)
        })
        .collect()
}

/// Breaks `text` at spaces into lines of at most `width` cells. A word longer than the width
/// keeps a line of its own and is cut where it is drawn.
pub(crate) fn wrap(text: &str, width: u16) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split(' ') {
        match lines.last_mut() {
            Some(line) if cells(line) + 1 + cells(word) <= width => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_owned()),
        }
    }
    lines
}

pub(crate) fn cells(text: &str) -> u16 {
    u16::try_from(text.width()).unwrap_or(u16::MAX)
}

pub(crate) fn blend(from: Color, to: Color, share: f64) -> Color {
    let (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) = (from, to) else {
        return from;
    };
    let mix = |a: u8, b: u8| (f64::from(a) + (f64::from(b) - f64::from(a)) * share).round() as u8;
    Color::Rgb(mix(r1, r2), mix(g1, g2), mix(b1, b2))
}

/// Writes `text` at a cell, and nothing when the cell lies outside the screen. The rows above
/// a panel sit at fixed offsets, so a short terminal puts some of them below its last row.
pub(crate) fn put(
    buffer: &mut Buffer,
    x: u16,
    y: u16,
    text: impl AsRef<str>,
    style: Style,
) -> (u16, u16) {
    put_within(buffer, x, y, text, usize::MAX, style)
}

pub(crate) fn put_within(
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
