use std::path::Path;

/// The C0 controls, DEL, and the C1 range.
fn control(c: char) -> bool {
    c < '\u{20}' || c == '\u{7f}' || ('\u{80}'..='\u{9f}').contains(&c)
}

/// The format characters that print nothing or reorder what follows: the soft hyphen, the
/// zero-width characters, the line and paragraph separators, the bidirectional embeddings,
/// overrides and isolates, the byte order mark, and the tag characters.
fn invisible(c: char) -> bool {
    matches!(
        c,
        '\u{ad}'
            | '\u{61c}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{e0000}'..='\u{e007f}'
    )
}

/// Makes text safe to write to a terminal. A file name is chosen by whoever wrote the file, not
/// by whoever destroys it. A control character inside one can retitle the window, move the
/// cursor, or hide the rest of the line, and an invisible or reordering character can make one
/// name display as another. A control character is below U+0100, so two hex digits always fit.
pub fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        if control(c) {
            escaped.push_str(&format!("\\x{:02x}", u32::from(c)));
        } else if invisible(c) {
            escaped.push_str(&format!("\\u{{{:x}}}", u32::from(c)));
        } else {
            escaped.push(c);
        }
    }
    escaped
}

/// A byte that is not valid UTF-8 becomes U+FFFD before the escape, so a raw 8-bit control
/// byte such as 0x9b never reaches the terminal either.
pub fn escape_path(path: &Path) -> String {
    escape(&path.to_string_lossy())
}
