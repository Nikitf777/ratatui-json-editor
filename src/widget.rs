//! The JSON tree widget.
//!
//! [`JsonEditor`] is a stateless widget (theme + display options) rendered
//! against a [`JsonEditorState`]. It only draws the tree — text input, modals
//! and scrollbars belong to the consumer, which drives scrolling through
//! [`ScrollMode`] and the state's viewport accessors.

use ratatui_core::buffer::Buffer;
use ratatui_core::layout::Rect;
use ratatui_core::text::{Line, Span};
use ratatui_core::widgets::StatefulWidget;

use crate::highlight::Theme;
use crate::json::{quote_string, Json};
use crate::state::JsonEditorState;
use crate::tree::{flatten, Row, RowContent};

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

        for (i, row) in rows.iter().skip(top).take(height).enumerate() {
            let rect = Rect::new(area.x, area.y + i as u16, area.width, 1);
            if top + i == state.cursor_line() {
                buf.set_style(rect, self.theme.cursor_line);
            }
            buf.set_line(
                area.x,
                rect.y,
                &Line::from(render_row(row, &self.theme)),
                area.width,
            );
        }
    }
}

fn render_row(row: &Row, theme: &Theme) -> Vec<Span<'static>> {
    let mut spans = vec![Span::raw("  ".repeat(row.depth))];
    match &row.content {
        RowContent::Container {
            key,
            is_object,
            empty,
        } => {
            if let Some(key) = key {
                push_key(&mut spans, key, theme);
            }
            let (open, close) = if *is_object { ("{", "}") } else { ("[", "]") };
            if *empty {
                spans.push(Span::styled(format!("{open}{close}"), theme.punct));
                push_comma(&mut spans, row.comma, theme);
            } else {
                spans.push(Span::styled(open, theme.punct));
            }
        }
        RowContent::Scalar { key, value } => {
            if let Some(key) = key {
                push_key(&mut spans, key, theme);
            }
            spans.push(scalar_span(value, theme));
            push_comma(&mut spans, row.comma, theme);
        }
        RowContent::Close { is_object } => {
            spans.push(Span::styled(
                if *is_object { "}" } else { "]" },
                theme.punct,
            ));
            push_comma(&mut spans, row.comma, theme);
        }
    }
    spans
}

fn push_key(spans: &mut Vec<Span<'static>>, key: &str, theme: &Theme) {
    spans.push(Span::styled(quote_string(key), theme.key));
    spans.push(Span::styled(":", theme.punct));
    spans.push(Span::raw(" "));
}

fn scalar_span(value: &Json, theme: &Theme) -> Span<'static> {
    match value {
        Json::String(s) => Span::styled(quote_string(s), theme.string),
        Json::Number(n) => Span::styled(n.to_string(), theme.number),
        Json::Bool(b) => Span::styled(b.to_string(), theme.boolean),
        _ => Span::styled("null", theme.null),
    }
}

fn push_comma(spans: &mut Vec<Span<'static>>, comma: bool, theme: &Theme) {
    if comma {
        spans.push(Span::styled(",", theme.punct));
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
    fn highlights_the_cursor_row() {
        let mut state = JsonEditorState::parse(r#"{"a": 1, "b": 2}"#).unwrap();
        state.cursor_down();
        let buf = render(&mut state, 30, 5);
        let theme = Theme::default();
        assert_eq!(buf[(0, 1)].style().bg, theme.cursor_line.bg);
        assert_ne!(buf[(0, 0)].style().bg, theme.cursor_line.bg);
    }

    #[test]
    fn follow_cursor_scrolls_the_state() {
        let mut state = JsonEditorState::parse("[1, 2, 3, 4, 5]").unwrap();
        for _ in 0..3 {
            state.cursor_down();
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
