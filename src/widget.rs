//! The JSON tree widget.
//!
//! [`JsonEditor`] is a stateless widget (theme + display options) rendered
//! against a [`JsonEditorState`]. It only draws the tree — text input, modals
//! and scrollbars belong to the consumer, which drives scrolling through
//! [`ScrollMode`] and the state's viewport accessors.

use ratatui_core::buffer::Buffer;
use ratatui_core::layout::Rect;
use ratatui_core::style::Style;
use ratatui_core::text::{Line, Span};
use ratatui_core::widgets::StatefulWidget;

use crate::highlight::Theme;
use crate::json::{quote_string, Json};
use crate::state::{Field, JsonEditorState};
use crate::tree::{flatten, Row, RowContent, INDENT_WIDTH};

/// Who scrolls the tree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScrollMode {
    /// The widget scrolls the minimum amount needed to keep the cursor row
    /// visible (the default).
    #[default]
    FollowCursor,
    /// The consumer owns scrolling through [`JsonEditorState::set_scroll`] and
    /// [`JsonEditorState::ensure_cursor_visible`] — for example to drive a
    /// scrollbar or respond to page keys.
    Manual,
}

/// Renders a [`JsonEditorState`] as a syntax-highlighted JSON tree.
///
/// ```
/// use ratatui_core::backend::TestBackend;
/// use ratatui_core::terminal::Terminal;
/// use ratatui_json_editor::{JsonEditor, JsonEditorState, ScrollMode};
///
/// let mut terminal = Terminal::new(TestBackend::new(30, 6)).unwrap();
/// let mut state = JsonEditorState::parse(r#"{"a": [1, 2]}"#).unwrap();
/// let widget = JsonEditor::new().scroll_mode(ScrollMode::Manual);
///
/// terminal.draw(|frame| {
///     frame.render_stateful_widget(&widget, frame.area(), &mut state);
/// }).unwrap();
/// assert_eq!(state.line_count(), 6);
/// ```
#[derive(Clone, Debug, Default)]
pub struct JsonEditor {
    theme: Theme,
    scroll_mode: ScrollMode,
}

impl JsonEditor {
    /// Creates a widget with the default theme and [`ScrollMode::FollowCursor`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the colors used to render JSON.
    pub fn theme(mut self, theme: Theme) -> Self {
        self.theme = theme;
        self
    }

    /// Sets who scrolls the tree.
    pub fn scroll_mode(mut self, scroll_mode: ScrollMode) -> Self {
        self.scroll_mode = scroll_mode;
        self
    }
}

impl StatefulWidget for &JsonEditor {
    type State = JsonEditorState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let height = area.height as usize;
        let rows = flatten(state.root());

        match self.scroll_mode {
            ScrollMode::FollowCursor => state.ensure_cursor_visible(height),
            ScrollMode::Manual => {}
        }
        let top = state.scroll().min(rows.len().saturating_sub(height));
        state.set_scroll(top);
        let cursor_line = state.cursor_line();
        let value_block = selected_value_block(&rows, cursor_line, state);

        for (i, row) in rows.iter().skip(top).take(height).enumerate() {
            let index = top + i;
            let rect = Rect::new(area.x, area.y + i as u16, area.width, 1);
            if index == cursor_line {
                buf.set_style(rect, self.theme.cursor_line);
            }
            let highlight = match value_block {
                Some((start, _)) if index == start => Highlight::Value,
                Some((_, end)) if index == end => Highlight::Close,
                Some((start, end)) if index > start && index < end => Highlight::Whole,
                _ if index == cursor_line => match state.selected_field() {
                    Field::Key => Highlight::Key,
                    Field::Value => Highlight::Value,
                },
                _ => Highlight::None,
            };
            buf.set_line(
                area.x,
                rect.y,
                &Line::from(render_row(row, &self.theme, highlight)),
                area.width,
            );
        }
    }
}

/// How much of a row the selection covers.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Highlight {
    /// Nothing is selected on this row.
    None,
    /// The key text on the cursor line.
    Key,
    /// The value on the cursor line: a scalar, `{}` / `[]`, or the open bracket
    /// of a container whose block is selected.
    Value,
    /// A whole row inside a selected container's block.
    Whole,
    /// The closing bracket row at the end of a selected container's block.
    Close,
}

/// The row range of the selected container value: from its open bracket row to
/// its closing bracket row.
fn selected_value_block(
    rows: &[Row],
    cursor_line: usize,
    state: &JsonEditorState,
) -> Option<(usize, usize)> {
    if state.selected_field() != Field::Value {
        return None;
    }
    let row = rows.get(cursor_line)?;
    if !matches!(&row.content, RowContent::Container { empty: false, .. }) {
        return None;
    }
    let depth = row.depth;
    let end = rows[cursor_line + 1..]
        .iter()
        .position(|r| matches!(&r.content, RowContent::Close { .. }) && r.depth == depth)?
        + cursor_line
        + 1;
    Some((cursor_line, end))
}

fn render_row(row: &Row, theme: &Theme, highlight: Highlight) -> Vec<Span<'static>> {
    let whole = highlight == Highlight::Whole;
    let key_on = whole || highlight == Highlight::Key;
    let value_on = whole || highlight == Highlight::Value;
    let close_on = whole || highlight == Highlight::Close;
    let mut spans = vec![Span::styled(
        " ".repeat(INDENT_WIDTH * row.depth),
        // On a closing row the indent is the whitespace just inside the
        // bracket, so a selected block covers it along with the bracket.
        patch(Style::default(), close_on, theme),
    )];
    match &row.content {
        RowContent::Container {
            key,
            is_object,
            empty,
        } => {
            if let Some(key) = key {
                push_key(&mut spans, key, theme, key_on, whole);
            }
            let (open, close) = if *is_object { ("{", "}") } else { ("[", "]") };
            if *empty {
                spans.push(Span::styled(
                    format!("{open}{close}"),
                    patch(theme.punct, value_on, theme),
                ));
                push_comma(&mut spans, row.comma, theme, whole);
            } else {
                spans.push(Span::styled(open, patch(theme.punct, value_on, theme)));
            }
        }
        RowContent::Scalar { key, value } => {
            if let Some(key) = key {
                push_key(&mut spans, key, theme, key_on, whole);
            }
            spans.push(scalar_span(value, theme, value_on));
            push_comma(&mut spans, row.comma, theme, whole);
        }
        RowContent::Close { is_object } => {
            spans.push(Span::styled(
                if *is_object { "}" } else { "]" },
                patch(theme.punct, close_on, theme),
            ));
            push_comma(&mut spans, row.comma, theme, whole);
        }
    }
    spans
}

fn patch(style: Style, on: bool, theme: &Theme) -> Style {
    if on {
        style.patch(theme.selection)
    } else {
        style
    }
}

fn push_key(spans: &mut Vec<Span<'static>>, key: &str, theme: &Theme, key_on: bool, rest_on: bool) {
    spans.push(Span::styled(
        quote_string(key),
        patch(theme.key, key_on, theme),
    ));
    spans.push(Span::styled(":", patch(theme.punct, rest_on, theme)));
    spans.push(Span::styled(" ", patch(Style::default(), rest_on, theme)));
}

fn scalar_span(value: &Json, theme: &Theme, selected: bool) -> Span<'static> {
    let (text, style) = match value {
        Json::String(s) => (quote_string(s), theme.string),
        Json::Number(n) => (n.to_string(), theme.number),
        Json::Bool(b) => (b.to_string(), theme.boolean),
        _ => ("null".to_string(), theme.null),
    };
    Span::styled(text, patch(style, selected, theme))
}

fn push_comma(spans: &mut Vec<Span<'static>>, comma: bool, theme: &Theme, on: bool) {
    if comma {
        spans.push(Span::styled(",", patch(theme.punct, on, theme)));
    }
}

#[cfg(test)]
mod tests {
    use ratatui_core::buffer::Buffer;

    use super::*;

    fn render(state: &mut JsonEditorState, width: u16, height: u16) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, width, height));
        let widget = JsonEditor::new();
        (&widget).render(Rect::new(0, 0, width, height), &mut buf, state);
        buf
    }

    fn text(buf: &Buffer) -> String {
        buf.content().iter().map(|cell| cell.symbol()).collect()
    }

    #[test]
    fn renders_the_tree_with_syntax_highlighting() {
        let mut state = JsonEditorState::parse(r#"{"a": "x"}"#).unwrap();
        let buf = render(&mut state, 30, 5);
        let text = text(&buf);
        assert!(text.contains("{"), "{text:?}");
        assert!(text.contains("\"a\""), "{text:?}");
        assert!(text.contains("\"x\""), "{text:?}");

        let theme = Theme::default();
        assert_eq!(buf[(2, 1)].style().fg, theme.key.fg);
        assert_eq!(buf[(8, 1)].style().fg, theme.string.fg);
    }

    #[test]
    fn highlights_the_selected_field() {
        let mut state = JsonEditorState::parse(r#"{"a": 1}"#).unwrap();
        state.select_down();
        let theme = Theme::default();

        state.select_key();
        let buf = render(&mut state, 30, 5);
        assert_eq!(buf[(3, 1)].style().bg, theme.selection.bg, "key highlighted");
        assert_ne!(buf[(7, 1)].style().bg, theme.selection.bg);

        state.select_value();
        let buf = render(&mut state, 30, 5);
        assert_eq!(buf[(7, 1)].style().bg, theme.selection.bg, "value highlighted");
        assert_ne!(buf[(3, 1)].style().bg, theme.selection.bg);
    }

    #[test]
    fn highlights_the_whole_container_value() {
        let mut state = JsonEditorState::parse(r#"{"a": [1, 2], "b": 3}"#).unwrap();
        state.select_down();
        assert!(state.select_value());
        let buf = render(&mut state, 30, 8);
        let theme = Theme::default();
        let sel = theme.selection.bg;

        // Row 1 is `  "a": [` — the key stays out, the block starts at `[`.
        assert_ne!(buf[(3, 1)].style().bg, sel, "key is not part of the value");
        assert_eq!(buf[(7, 1)].style().bg, sel, "open bracket highlighted");

        // Rows 2 and 3 are the array's elements — fully highlighted.
        assert_eq!(buf[(0, 2)].style().bg, sel, "inner rows highlighted");
        assert_eq!(buf[(4, 2)].style().bg, sel);
        assert_eq!(buf[(4, 3)].style().bg, sel);

        // Row 4 is `  ]` — the block ends at the close bracket, and the
        // indent before it is inside the brackets.
        assert_eq!(buf[(2, 4)].style().bg, sel, "close bracket highlighted");
        assert_eq!(buf[(0, 4)].style().bg, sel, "indent before the bracket highlighted");
        assert_ne!(buf[(2, 5)].style().bg, sel, "the next entry stays out");

        // Row 5 is `  "b": 3` — outside the block.
        assert_ne!(buf[(3, 5)].style().bg, sel);
        assert_ne!(buf[(7, 5)].style().bg, sel);
    }

    #[test]
    fn highlights_the_cursor_row() {
        let mut state = JsonEditorState::parse(r#"{"a": 1, "b": 2}"#).unwrap();
        state.select_down();
        let buf = render(&mut state, 30, 5);
        let theme = Theme::default();
        assert_eq!(buf[(0, 1)].style().bg, theme.cursor_line.bg);
        assert_ne!(buf[(0, 0)].style().bg, theme.cursor_line.bg);
    }

    #[test]
    fn follow_cursor_scrolls_the_state() {
        let mut state = JsonEditorState::parse("[1, 2, 3, 4, 5]").unwrap();
        for _ in 0..3 {
            state.select_down();
        }
        let _ = render(&mut state, 30, 3);
        assert_eq!(state.cursor_line(), 3);
        assert_eq!(state.scroll(), 1, "cursor kept in the 3-row viewport");
    }

    #[test]
    fn manual_mode_respects_consumer_scrolling() {
        let mut state = JsonEditorState::parse("[1, 2, 3, 4, 5]").unwrap();
        state.set_scroll(3);
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 2));
        let widget = JsonEditor::new().scroll_mode(ScrollMode::Manual);
        (&widget).render(Rect::new(0, 0, 30, 2), &mut buf, &mut state);
        assert_eq!(state.scroll(), 3, "the widget does not override the scroll");
        assert!(text(&buf).contains('4'), "{:?}", text(&buf));
    }

    #[test]
    fn scroll_is_clamped_to_the_document() {
        let mut state = JsonEditorState::parse("[1]").unwrap();
        state.set_scroll(99);
        let _ = render(&mut state, 30, 5);
        assert_eq!(state.scroll(), 0);
    }
}
