use std::path::Path;

use icu_properties::props::{DefaultIgnorableCodePoint, GeneralCategory};
use icu_properties::{CodePointMapData, CodePointSetData};

/// The C0 controls, DEL, and the C1 range.
fn control(c: char) -> bool {
    c < '\u{20}' || c == '\u{7f}' || ('\u{80}'..='\u{9f}').contains(&c)
}

/// The characters that print nothing or reorder what follows, read from the Unicode data so the
/// set has one owner: every default ignorable code point, the format category, and the line and
/// paragraph separators, which belong to neither but end a line where the name does not.
fn invisible(c: char) -> bool {
    CodePointSetData::new::<DefaultIgnorableCodePoint>().contains(c)
        || matches!(
            CodePointMapData::<GeneralCategory>::new().get(c),
            GeneralCategory::Format
                | GeneralCategory::LineSeparator
                | GeneralCategory::ParagraphSeparator
        )
}

/// Makes text safe to write to a terminal. A file name is chosen by whoever wrote the file, not
/// by whoever destroys it. A control character inside one can retitle the window, move the
/// cursor, or hide the rest of the line, and an invisible or reordering character can make one
/// name display as another. A control character is below U+0100, so two hex digits always fit.
///
/// The backslash is escaped too, so a name that spells out the escape of another name still
/// displays differently from it.
pub fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        if c == '\\' {
            escaped.push_str("\\\\");
        } else if control(c) {
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
