//! Drawing: the menu bar, the text input line, and the JSON editor panel.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Block;
use ratatui_json_editor::{
    Field, JsonEditor, Run, ScrollMode, clip_spans, overlay, runs_to_spans, styled_runs,
};
use ratatui_textarea::DataCursor;
use tui_menu::Menu;
use tui_popup::Popup;
use tui_scrollbar::{GlyphSet, ScrollBar, ScrollBarArrows, ScrollLengths};

use crate::app::{App, Mode};
use crate::format::compact;

pub(crate) const TAB_LEN: usize = 2;

pub(crate) fn draw(frame: &mut Frame, app: &mut App) {
    let [menu, input, editor] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Min(3),
    ])
    .areas(frame.area());
    app.menu_rect = menu;

    render_input_line(frame, app, input);
    render_editor(frame, app, editor);
    // Drawn last so its dropdowns float above the panels.
    frame.render_stateful_widget(Menu::new(), menu, &mut app.menu);

    if app.popup {
        let popup = Popup::new("q: quit without saving\nCtrl+S: save and quit\nEsc: cancel")
            .title(" Unsaved changes ");
        frame.render_widget(popup, frame.area());
    }
}

/// The text input: a framed full-width box, one text row tall. While editing
/// it shows the live `ratatui-textarea` buffer; otherwise it mirrors what is
/// selected. The title carries the mode and any message.
fn render_input_line(frame: &mut Frame, app: &mut App, area: Rect) {
    let label = match (app.mode, app.state.selected_field()) {
        (Mode::Edit, Field::Value) => " Edit value as JSON text ",
        (Mode::Edit, Field::Key) => " Edit key as plain text ",
        (Mode::Normal, Field::Value) => " JSON text (value selected) ",
        (Mode::Normal, Field::Key) => " Plain text (key selected) ",
    };
    let mut title = vec![Span::styled(label, app.theme.popup_title)];
    if let Some(message) = &app.message {
        title.push(Span::styled(
            format!(" ⚠ {message} "),
            Style::new().fg(Color::LightRed),
        ));
    }
    let block = Block::bordered()
        .title(Line::from(title))
        .border_style(app.theme.popup);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.input_rect = inner;

    if app.mode == Mode::Edit {
        render_input(frame, app, inner);
    } else if inner.height > 0 {
        let text = match app.state.selected_field() {
            Field::Key => app.state.edit(),
            Field::Value => compact(app.state.selected()),
        };
        let chars: Vec<char> = text.chars().collect();
        let runs = styled_runs(&text, &app.theme);
        let spans = clip_spans(
            runs_to_spans(&chars, &runs, TAB_LEN),
            0,
            inner.width as usize,
        );
        frame.render_widget(Line::from(spans), inner);
    }
}

fn render_editor(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::bordered().title(" JSON editor ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [tree_area, bar_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    let [tree_area, hbar_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(tree_area);
    app.view_height = tree_area.height as usize;
    app.tree_rect = tree_area;
    app.scrollbar_rect = bar_area;
    app.hbar_rect = hbar_area;

    let widget = JsonEditor::new()
        .theme(app.theme.clone())
        .scroll_mode(ScrollMode::Manual);
    frame.render_stateful_widget(&widget, tree_area, &mut app.state);

    let vbar = scrollbar(
        app.state.line_count(),
        tree_area.height as usize,
        app.state.scroll(),
    );
    frame.render_widget(&vbar, bar_area);
    let hbar = h_scrollbar(
        app.state.content_width(),
        tree_area.width as usize,
        app.state.scroll_x(),
    );
    frame.render_widget(&hbar, hbar_area);
}

/// Renders the `ratatui-textarea` buffer with syntax highlighting.
fn render_input(frame: &mut Frame, app: &mut App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let height = area.height as usize;
    let width = area.width as usize;
    let field = app.state.selected_field();
    let theme = app.theme.clone();
    let lines = app.textarea.lines();
    let DataCursor(cursor_row, cursor_col) = app.textarea.cursor();
    let selection = app.textarea.selection_range();

    let mut top = app.edit_row;
    if cursor_row < top {
        top = cursor_row;
    }
    if cursor_row >= top + height {
        top = cursor_row + 1 - height;
    }
    app.edit_row = top;

    let cursor_x = display_width(
        &lines
            .get(cursor_row)
            .map(|line| line.chars().take(cursor_col).collect::<String>())
            .unwrap_or_default(),
    );
    let mut left = app.edit_col;
    if cursor_x < left {
        left = cursor_x;
    }
    if cursor_x >= left + width {
        left = cursor_x + 1 - width;
    }
    app.edit_col = left;

    let buf = frame.buffer_mut();
    for row in 0..height {
        let Some(line) = lines.get(top + row) else {
            break;
        };
        let chars: Vec<char> = line.chars().collect();
        let mut runs: Vec<Run> = match field {
            Field::Value => styled_runs(line, &theme),
            Field::Key => vec![(0, chars.len(), theme.key)],
        };
        let line_index = top + row;
        if let Some(((start_row, start_col), (end_row, end_col))) = selection
            && start_row <= line_index
            && line_index <= end_row
        {
            let start = if line_index == start_row {
                start_col
            } else {
                0
            };
            let end = if line_index == end_row {
                end_col
            } else {
                chars.len()
            };
            runs = overlay(
                runs,
                (start.min(chars.len()), end.min(chars.len())),
                theme.selection,
            );
        }
        let at_end = line_index == cursor_row && cursor_col >= chars.len();
        if line_index == cursor_row && cursor_col < chars.len() {
            runs = overlay(runs, (cursor_col, cursor_col + 1), theme.cursor);
        }
        let mut spans = runs_to_spans(&chars, &runs, TAB_LEN);
        if at_end {
            spans.push(Span::styled(" ", theme.cursor));
        }
        let spans = clip_spans(spans, left, width);
        let rect = Rect::new(area.x, area.y + row as u16, area.width, 1);
        if line_index == cursor_row {
            buf.set_style(rect, theme.cursor_line);
        }
        buf.set_line(area.x, rect.y, &Line::from(spans), area.width);
    }
}

// -- scrollbars and text measurement ----------------------------------------

pub(crate) fn scrollbar(content_len: usize, viewport_len: usize, offset: usize) -> ScrollBar {
    ScrollBar::vertical(ScrollLengths {
        content_len,
        viewport_len,
    })
    .offset(offset)
    .arrows(ScrollBarArrows::Both)
    .glyph_set(GlyphSet::box_drawing())
}

pub(crate) fn h_scrollbar(content_len: usize, viewport_len: usize, offset: usize) -> ScrollBar {
    ScrollBar::horizontal(ScrollLengths {
        content_len,
        viewport_len,
    })
    .offset(offset)
    .arrows(ScrollBarArrows::Both)
    .glyph_set(GlyphSet::box_drawing())
}

pub(crate) fn display_width(text: &str) -> usize {
    text.chars()
        .map(|c| {
            if c == '\t' {
                TAB_LEN
            } else {
                unicode_width::UnicodeWidthChar::width(c).unwrap_or(0)
            }
        })
        .sum()
}

/// The character index at display column `col` of `text`.
pub(crate) fn char_at(text: &str, col: usize) -> usize {
    let mut width = 0;
    for (i, c) in text.chars().enumerate() {
        if width >= col {
            return i;
        }
        width += if c == '\t' {
            TAB_LEN
        } else {
            unicode_width::UnicodeWidthChar::width(c).unwrap_or(0)
        };
    }
    text.chars().count()
}
