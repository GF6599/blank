use blank::terminal::escape;

/// Every character that prints nothing or reorders what follows is escaped.
///
/// Given: a name around one character from each range of the format category and of the
/// default ignorable code points, the line and paragraph separators, and the characters the
/// first list already held.
/// When: it is escaped.
/// Then: each character shows as its code point, and the letters around it are left alone.
///
/// Why it matters: a character that prints nothing makes one name display as another, so the
/// operator confirms a name that is not the one the run destroys.
#[test]
fn every_character_that_prints_nothing_or_reorders_is_escaped() {
    for code in [
        0x00ad, 0x0600, 0x0605, 0x061c, 0x06dd, 0x070f, 0x0890, 0x0891, 0x08e2, 0x180e, 0x200b,
        0x200f, 0x2028, 0x2029, 0x202e, 0x2060, 0x2065, 0x2066, 0xfe00, 0xfe0f, 0xfeff, 0xfff9,
        0x110bd, 0x110cd, 0x13430, 0x1343f, 0x1bca0, 0x1bca3, 0x1d173, 0x1d17a, 0xe0001, 0xe0100,
        0xe01ef,
    ] {
        // Given:
        let character = char::from_u32(code).expect("every code here is a character");
        let name = format!("a{character}b");

        // When:
        let shown = escape(&name);

        // Then:
        assert_eq!(shown, format!("a\\u{{{code:x}}}b"), "U+{code:04X}");
    }
}

/// A name in an ordinary script, and an emoji with its variation selector, display as decided.
///
/// Given: a name in Japanese, one with an accented Latin letter, and one with a heart emoji.
/// When: each is escaped.
/// Then: the first two are unchanged, and the heart keeps its glyph with the variation selector
/// shown as its code point.
///
/// Why it matters: an escape that reaches ordinary letters makes every name in those scripts
/// unreadable, and the variation selector is escaped on purpose, because it prints nothing.
#[test]
fn ordinary_letters_stay_and_a_variation_selector_shows() {
    // Given:
    let names = ["記憶.txt", "café.txt", "\u{2764}\u{fe0f}.txt"];

    // When:
    let shown: Vec<String> = names.iter().map(|name| escape(name)).collect();

    // Then:
    assert_eq!(shown, ["記憶.txt", "café.txt", "\u{2764}\\u{fe0f}.txt"]);
}
