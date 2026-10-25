use std::sync::LazyLock;

use syntect::easy::ScopeRangeIterator;
use syntect::highlighting::ScopeSelectors;
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

use crate::terminal::escape;

/// What a span of source text is, which decides its colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token {
    Plain,
    Comment,
    Keyword,
    String,
    /// A number, or a word the language defines, such as `true` or `null`.
    Constant,
    Function,
    Type,
    Key,
    /// Punctuation, and an operator such as `=`.
    Punctuation,
}

/// The line of a highlighted preview, as its spans, each escaped.
pub type Spans = Vec<(String, Token)>;

/// The scopes of each token, in the selector syntax of the grammars. Where several match a span,
/// the most specific one wins, so a JSON key, which is also a string, reads as a key.
const SELECTORS: [(&str, Token); 8] = [
    ("comment, punctuation.definition.comment", Token::Comment),
    ("keyword, storage", Token::Keyword),
    ("string, punctuation.definition.string", Token::String),
    ("constant.numeric, constant.language", Token::Constant),
    ("entity.name.function, support.function", Token::Function),
    (
        "entity.name.type, entity.name.class, support.type, support.class",
        Token::Type,
    ),
    (
        "meta.structure.dictionary.key string, meta.mapping.key string",
        Token::Key,
    ),
    ("punctuation, keyword.operator", Token::Punctuation),
];

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);

static TOKENS: LazyLock<Vec<(ScopeSelectors, Token)>> = LazyLock::new(|| {
    SELECTORS
        .iter()
        .map(|&(selector, token)| (selector.parse().expect("a valid selector"), token))
        .collect()
});

/// The lines of `text` highlighted by the grammar of `extension`, with every tab replaced by
/// `tab`. Nothing comes back for an extension with no grammar, for text the grammar cannot
/// parse, or for no text.
pub fn highlight(extension: &str, text: &str, tab: &str) -> Option<Vec<Spans>> {
    let syntax = grammar(extension)?;
    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut lines = Vec::new();
    for line in LinesWithEndings::from(text) {
        let operations = state.parse_line(line, &SYNTAXES).ok()?;
        // The grammar reads the line ending, and the preview drops it as `str::lines` does.
        let ending = line
            .strip_suffix("\r\n")
            .or_else(|| line.strip_suffix('\n'))
            .map_or(line.len(), str::len);
        let mut spans: Spans = Vec::new();
        for (range, operation) in ScopeRangeIterator::new(&operations, line) {
            stack.apply(operation).ok()?;
            let part = &line[range.start.min(ending)..range.end.min(ending)];
            if part.is_empty() {
                continue;
            }
            let token = token(&stack);
            let part = escape(&part.replace('\t', tab));
            match spans.last_mut() {
                Some((joined, last)) if *last == token => joined.push_str(&part),
                _ => spans.push((part, token)),
            }
        }
        lines.push(spans);
    }
    (!lines.is_empty()).then_some(lines)
}

/// Plain text has a grammar of its own, which marks nothing.
fn grammar(extension: &str) -> Option<&'static SyntaxReference> {
    let syntax = SYNTAXES.find_syntax_by_extension(extension)?;
    (syntax.name != SYNTAXES.find_syntax_plain_text().name).then_some(syntax)
}

fn token(stack: &ScopeStack) -> Token {
    TOKENS
        .iter()
        .filter_map(|(selectors, token)| Some((selectors.does_match(stack.as_slice())?, *token)))
        .max_by_key(|(power, _)| *power)
        .map_or(Token::Plain, |(_, token)| token)
}
