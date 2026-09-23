//! Syntax highlighting for JSON text.
//!
//! [`lex_line`] is a small JSON lexer that assigns a [`TokenKind`] to every
//! character of a line; [`Theme`] maps those kinds to colors. The same lexer
//! powers the tree view, the live output pane and the text area shown in edit
//! mode, so colors are consistent everywhere.

use ratatui_core::style::{Color, Modifier, Style};
use ratatui_core::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

/// Colors and emphasis used to render JSON.
#[derive(Clone, Debug)]
pub struct Theme {
    /// Object keys
    pub key: Style,
    /// String values
    pub string: Style,
    /// Numbers
    pub number: Style,
    /// `true` / `false`
    pub boolean: Style,
    /// `null`
    pub null: Style,
    /// Brackets, braces, commas and colons
    pub punct: Style,
    /// Text that is not (yet) valid JSON
    pub error: Style,
    /// The character under the cursor in edit mode
    pub cursor: Style,
    /// Background of the line under the tree cursor / edit cursor
    pub cursor_line: Style,
    /// Background of selected text in edit mode
    pub selection: Style,
    /// Style of the edit popup frame
    pub popup: Style,
    /// Style of the edit popup title
    pub popup_title: Style,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            key: Style::default().fg(Color::LightCyan),
            string: Style::default().fg(Color::LightGreen),
            number: Style::default().fg(Color::LightYellow),
            boolean: Style::default().fg(Color::LightMagenta),
            null: Style::default().fg(Color::Gray),
            punct: Style::default().fg(Color::DarkGray),
            error: Style::default().fg(Color::LightRed).add_modifier(Modifier::BOLD),
            cursor: Style::default().add_modifier(Modifier::REVERSED),
            cursor_line: Style::default().bg(Color::Rgb(40, 42, 54)),
            selection: Style::default().bg(Color::Rgb(70, 70, 100)),
            popup: Style::default().fg(Color::Cyan),
            popup_title: Style::default().fg(Color::LightCyan).add_modifier(Modifier::BOLD),
        }
    }
}

impl Theme {
    /// The style for a token kind.
    pub fn style(&self, kind: TokenKind) -> Style {
        match kind {
            TokenKind::Whitespace => Style::default(),
            TokenKind::Punct => self.punct,
            TokenKind::Key => self.key,
            TokenKind::String => self.string,
            TokenKind::Number => self.number,
            TokenKind::Bool => self.boolean,
            TokenKind::Null => self.null,
            TokenKind::Error => self.error,
        }
    }
}

/// What a piece of a line means in JSON syntax.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    /// Spaces and tabs
    Whitespace,
    /// `{`, `}`, `[`, `]`, `,`, `:`
    Punct,
    /// A string used as an object key
    Key,
    /// A string value
    String,
    /// A number
    Number,
    /// `true` or `false`
    Bool,
    /// `null`
    Null,
    /// Anything that is not valid JSON
    Error,
}

/// A lexed piece of a line, in character offsets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    /// First character of the token.
    pub start: usize,
    /// Character after the last character of the token.
    pub end: usize,
    /// What the token means.
    pub kind: TokenKind,
}

/// A styled character range of a line (character offsets).
pub type Run = (usize, usize, Style);

/// Lexes one line of JSON-ish text.
///
/// The lexer is deliberately permissive: unterminated or malformed parts are
/// tagged as [`TokenKind::Error`] instead of failing, so partially typed JSON
/// can still be highlighted while the user edits it. Since JSON strings cannot
/// span line breaks, per-line lexing is exact for well-formed documents.
pub fn lex_line(line: &str) -> Vec<Token> {
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < n {
        let start = i;
        let kind = match chars[i] {
            ' ' | '\t' => {
                while i < n && (chars[i] == ' ' || chars[i] == '\t') {
                    i += 1;
                }
                TokenKind::Whitespace
            }
            '{' | '}' | '[' | ']' | ',' | ':' => {
                i += 1;
                TokenKind::Punct
            }
            '"' => {
                i += 1;
                let mut closed = false;
                while i < n {
                    match chars[i] {
                        '\\' => i = (i + 2).min(n),
                        '"' => {
                            i += 1;
                            closed = true;
                            break;
                        }
                        _ => i += 1,
                    }
                }
                if closed {
                    TokenKind::String
                } else {
                    TokenKind::Error
                }
            }
            '-' | '0'..='9' => {
                while i < n
                    && (matches!(chars[i], '-' | '+' | '.' | 'e' | 'E') || chars[i].is_ascii_digit())
                {
                    i += 1;
                }
                let text: String = chars[start..i].iter().collect();
                if crate::json::Number::new(&text).is_some() {
                    TokenKind::Number
                } else {
                    TokenKind::Error
                }
            }
            c if c.is_ascii_alphabetic() => {
                while i < n && chars[i].is_ascii_alphabetic() {
                    i += 1;
                }
                match &chars[start..i] {
                    ['t', 'r', 'u', 'e'] | ['f', 'a', 'l', 's', 'e'] => TokenKind::Bool,
                    ['n', 'u', 'l', 'l'] => TokenKind::Null,
                    _ => TokenKind::Error,
                }
            }
            _ => {
                i += 1;
                TokenKind::Error
            }
        };
        tokens.push(Token { start, end: i, kind });
    }

    // A string directly followed by a colon is an object key.
    for idx in 0..tokens.len() {
        if tokens[idx].kind != TokenKind::String {
            continue;
        }
        let next = tokens[idx + 1..].iter().find(|t| t.kind != TokenKind::Whitespace);
        if matches!(next, Some(t) if t.kind == TokenKind::Punct) {
            let between: String = chars[tokens[idx].end..next.unwrap().start].iter().collect();
            if between.chars().all(|c| c == ' ' || c == '\t')
                && chars.get(next.unwrap().start) == Some(&':')
            {
                tokens[idx].kind = TokenKind::Key;
            }
        }
    }
    tokens
}

/// Styles a line as runs of characters sharing one style.
pub fn styled_runs(line: &str, theme: &Theme) -> Vec<Run> {
    lex_line(line)
        .into_iter()
        .map(|t| (t.start, t.end, theme.style(t.kind)))
        .collect()
}

/// Splits runs so `range` becomes its own runs with `style` patched on top.
pub fn overlay(mut runs: Vec<Run>, range: (usize, usize), style: Style) -> Vec<Run> {
    let (start, end) = range;
    if start >= end {
        return runs;
    }
    let mut out = Vec::with_capacity(runs.len() + 2);
    for (s, e, st) in runs.drain(..) {
        if e <= start || s >= end {
            out.push((s, e, st));
            continue;
        }
        if s < start {
            out.push((s, start, st));
        }
        let mid_start = s.max(start);
        let mid_end = e.min(end);
        out.push((mid_start, mid_end, st.patch(style)));
        if end < e {
            out.push((end, e, st));
        }
    }
    out
}

/// Turns character runs into spans, expanding tabs to `tab_len` spaces.
pub fn runs_to_spans(chars: &[char], runs: &[Run], tab_len: usize) -> Vec<Span<'static>> {
    runs.iter()
        .map(|&(start, end, style)| {
            let mut text = String::new();
            for &c in &chars[start.min(chars.len())..end.min(chars.len())] {
                if c == '\t' {
                    text.push_str(&" ".repeat(tab_len));
                } else {
                    text.push(c);
                }
            }
            Span::styled(text, style)
        })
        .collect()
}

/// Clips spans to the display window `[left, left + width)` (in terminal cells).
pub fn clip_spans(spans: Vec<Span<'static>>, left: usize, width: usize) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut col = 0usize;
    for span in spans {
        let span_width = span.content.width();
        if col + span_width <= left || col >= left + width {
            col += span_width;
            continue;
        }
        let mut visible = String::new();
        for c in span.content.chars() {
            let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
            let next = col + w;
            if next > left && col < left + width {
                visible.push(c);
            }
            col = next;
        }
        if !visible.is_empty() {
            out.push(Span::styled(visible, span.style));
        }
    }
    out
}

/// Highlights a whole JSON text (possibly multi-line) as lines of spans.
pub fn highlight_json(text: &str, theme: &Theme) -> Vec<Line<'static>> {
    text.split('\n')
        .map(|line| {
            let chars: Vec<char> = line.chars().collect();
            let runs = styled_runs(line, theme);
            Line::from(runs_to_spans(&chars, &runs, 2))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(line: &str) -> Vec<(String, TokenKind)> {
        let chars: Vec<char> = line.chars().collect();
        lex_line(line)
            .into_iter()
            .map(|t| (chars[t.start..t.end].iter().collect(), t.kind))
            .collect()
    }

    #[test]
    fn lexes_keys_values_and_punctuation() {
        assert_eq!(
            kinds(r#"  "a": [1, "x", true, null],"#),
            vec![
                ("  ".into(), TokenKind::Whitespace),
                ("\"a\"".into(), TokenKind::Key),
                (":".into(), TokenKind::Punct),
                (" ".into(), TokenKind::Whitespace),
                ("[".into(), TokenKind::Punct),
                ("1".into(), TokenKind::Number),
                (",".into(), TokenKind::Punct),
                (" ".into(), TokenKind::Whitespace),
                ("\"x\"".into(), TokenKind::String),
                (",".into(), TokenKind::Punct),
                (" ".into(), TokenKind::Whitespace),
                ("true".into(), TokenKind::Bool),
                (",".into(), TokenKind::Punct),
                (" ".into(), TokenKind::Whitespace),
                ("null".into(), TokenKind::Null),
                ("]".into(), TokenKind::Punct),
                (",".into(), TokenKind::Punct),
            ]
        );
    }

    #[test]
    fn strings_only_count_as_keys_before_a_colon() {
        assert_eq!(kinds(r#""a"  : 1"#)[0].1, TokenKind::Key);
        assert_eq!(kinds(r#"["a" : 1]"#)[1].1, TokenKind::Key);
        assert_eq!(kinds(r#""a" 1"#)[0].1, TokenKind::String);
    }

    #[test]
    fn tags_invalid_text_as_error() {
        assert_eq!(kinds("01")[0].1, TokenKind::Error);
        assert_eq!(kinds("hello")[0].1, TokenKind::Error);
        assert_eq!(kinds("\"open")[0].1, TokenKind::Error);
        assert_eq!(kinds("1.2.3")[0].1, TokenKind::Error);
    }

    #[test]
    fn runs_cover_the_whole_line() {
        let line = r#"  "k": {"n": -1.5e2, "s": "x\ty", "b": false, "z": null}"#;
        let chars: Vec<char> = line.chars().collect();
        let runs = styled_runs(line, &Theme::default());
        assert_eq!(runs.first().unwrap().0, 0);
        assert_eq!(runs.last().unwrap().1, chars.len());
        for pair in runs.windows(2) {
            assert_eq!(pair[0].1, pair[1].0, "runs must not gap or overlap");
        }
    }

    #[test]
    fn overlay_splits_and_patches() {
        let runs = vec![(0usize, 6usize, Style::default())];
        let styled = overlay(runs, (2, 4), Style::default().bg(Color::Red));
        assert_eq!(styled.len(), 3);
        assert_eq!((styled[0].0, styled[0].1), (0, 2));
        assert_eq!((styled[1].0, styled[1].1), (2, 4));
        assert_eq!(styled[1].2.bg, Some(Color::Red));
        assert_eq!(styled[0].2.bg, None);
    }

    #[test]
    fn clip_spans_respects_the_window() {
        let spans = vec![Span::raw("abcdef")];
        let clipped = clip_spans(spans, 2, 3);
        assert_eq!(clipped.len(), 1);
        assert_eq!(&*clipped[0].content, "cde");
    }
}
