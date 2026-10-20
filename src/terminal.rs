use std::path::Path;

fn unsafe_for_terminal(c: char) -> bool {
    c < '\u{20}' || c == '\u{7f}' || ('\u{80}'..='\u{9f}').contains(&c)
}

/// Makes text safe to write to a terminal. A file name is chosen by whoever wrote the file, not
/// by whoever destroys it, and a control character inside one can retitle the window, move the
/// cursor, or hide the rest of the line. Every escaped character is below U+0100, so two hex
/// digits always fit.
pub fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        if unsafe_for_terminal(c) {
            escaped.push_str(&format!("\\x{:02x}", u32::from(c)));
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
