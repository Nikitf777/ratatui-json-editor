//! Interactive demo: one way to compose the JSON editor from small pieces.
//!
//! ```text
//! cargo run --example demo [file.json]
//! ```
//!
//! The library provides only `JsonEditorState` (document + operations) and the
//! `JsonEditor` tree widget. Everything else lives right here in this file:
//!
//! * two modes (normal / edit) and their keymap,
//! * the text input, a framed full-width box (one text row) above both panels,
//!   rendered with `ratatui-textarea` and syntax highlighted with the
//!   library's span helpers (always visible: it mirrors the selected node's
//!   JSON text and becomes the edit buffer in edit mode),
//! * a `tui-scrollbar` scrollbar (fractional thumb), driven by the state's
//!   viewport accessors with `ScrollMode::Manual`.
//!
//! The state's key/value selection is the single source of truth for what is
//! being edited: edit mode always edits exactly the selected field, and the
//! tree highlights it while you type.
//!
//! Without an argument the demo starts from a small sample document; with one
//! it loads the given file. On exit the (always valid) JSON is printed to
//! stdout.
//!
//! Mouse: click in the tree to select the key or value under the pointer,
//! click the input line to place the text cursor, wheel to scroll, and the
//! scrollbar handles clicks, arrows and thumb drags.
//!
//! Keys in normal mode:
//!
//! ```text
//! type a char  replace the text and edit     F2          edit with text selected
//! Enter / Shift+Enter  select down / up     Tab / Shift+Tab  select left / right
//! j/k or ↓/↑   select down / up             h/l or ←/→  select left / right
//! e / r        edit value / edit key        a           add entry (key first)
//! d or x       delete entry                 J / K       reorder among siblings
//! PgUp/PgDn    scroll                       q / Esc     quit
//! ```
//!
//! Keys in edit mode: plain typing through `ratatui-textarea`, so all of its
//! operations work (`Ctrl+U` undo, `Ctrl+R` redo, `Ctrl+W` delete word,
//! `Ctrl+K`/`Ctrl+J` delete to end/start of line, word motions, yank/paste,
//! ...), plus `Enter` / `Shift+Enter` to commit and select down / up, `Tab` /
//! `Shift+Tab` to commit and select left / right (like moving between cells in
//! a spreadsheet), and `Esc` to cancel. Invalid text is never committed.

use std::io;

use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyEventKind, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use ratatui_json_editor::{
    clip_spans, highlight_json, overlay, quote_string, runs_to_spans, styled_runs, EditError, Field,
    Json, JsonEditor, JsonEditorState, Run, ScrollMode, Theme,
};
use ratatui_textarea::{CursorMove, DataCursor, Input, Key, TextArea};
use tui_scrollbar::{
    GlyphSet, PointerButton, PointerEvent, PointerEventKind, ScrollAxis, ScrollBar, ScrollBarArrows,
    ScrollBarInteraction, ScrollCommand, ScrollEvent, ScrollLengths, ScrollWheel,
};

const SAMPLE: &str = r#"{
  "name": "ratatui-json-editor",
  "version": "0.1.0",
  "tags": ["json", "tui", "ratatui"],
  "settings": {
    "tab_width": 2,
    "line_numbers": true,
    "theme": null
  }
}"#;

const TAB_LEN: usize = 2;

fn main() -> io::Result<()> {
    let state = match std::env::args().nth(1) {
        Some(path) => {
            let text = std::fs::read_to_string(&path)?;
            JsonEditorState::parse(&text)
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?
        }
        None => JsonEditorState::parse(SAMPLE).expect("the sample is valid JSON"),
    };

    let mut terminal = ratatui::init();
    let mut app = App::new(state);
    let result = run(&mut terminal, &mut app);
    ratatui::restore();
    result?;

    println!("{}", pretty(app.state.root()));
    Ok(())
}

fn run(terminal: &mut DefaultTerminal, app: &mut App) -> io::Result<()> {
    execute!(io::stdout(), EnableBracketedPaste, EnableMouseCapture)?;
    let result = event_loop(terminal, app);
    execute!(io::stdout(), DisableBracketedPaste, DisableMouseCapture)?;
    result
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> io::Result<()> {
    loop {
        terminal.draw(|frame| draw(frame, app))?;
        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if app.handle_key(Input::from(key)) {
                    return Ok(());
                }
            }
            Event::Paste(text) if app.mode == Mode::Edit => {
                app.textarea.insert_str(&text);
            }
            Event::Mouse(mouse) => app.handle_mouse(mouse),
            _ => {}
        }
    }
}

// -- application state (modes, input, messages) -----------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    Edit,
}

struct App {
    state: JsonEditorState,
    theme: Theme,
    textarea: TextArea<'static>,
    mode: Mode,
    message: Option<String>,
    view_height: usize,
    edit_row: usize,
    edit_col: usize,
    input_rect: Rect,
    tree_rect: Rect,
    scrollbar_rect: Rect,
    scrollbar_interaction: ScrollBarInteraction,
}

impl App {
    fn new(state: JsonEditorState) -> Self {
        Self {
            state,
            theme: Theme::default(),
            textarea: TextArea::default(),
            mode: Mode::Normal,
            message: None,
            view_height: 1,
            edit_row: 0,
            edit_col: 0,
            input_rect: Rect::default(),
            tree_rect: Rect::default(),
            scrollbar_rect: Rect::default(),
            scrollbar_interaction: ScrollBarInteraction::new(),
        }
    }

    /// Returns `true` when the application should quit.
    fn handle_key(&mut self, input: Input) -> bool {
        match self.mode {
            Mode::Normal => self.normal_key(input),
            Mode::Edit => {
                self.edit_input(input);
                false
            }
        }
    }

    fn normal_key(&mut self, input: Input) -> bool {
        self.message = None;
        match (input.key, input.ctrl, input.alt, input.shift) {
            (Key::Char('q') | Key::Esc, false, false, _) | (Key::Char('c'), true, _, _) => {
                return true;
            }
            (Key::Char('j') | Key::Down, false, false, _) => {
                self.state.select_down();
            }
            (Key::Char('k') | Key::Up, false, false, _) => {
                self.state.select_up();
            }
            (Key::Char('h') | Key::Left, false, false, _) => {
                self.state.select_left();
            }
            (Key::Char('l') | Key::Right, false, false, _) => {
                self.state.select_right();
            }
            (Key::Tab, false, false, false) => {
                self.state.select_left();
            }
            (Key::Tab, false, false, true) => {
                self.state.select_right();
            }
            (Key::Enter, false, false, false) => {
                self.state.select_down();
            }
            (Key::Enter, false, false, true) => {
                self.state.select_up();
            }
            (Key::F(2), false, false, _) => {
                self.begin_edit();
            }
            (Key::Char('e'), false, false, _) => {
                self.state.select_value();
                self.begin_edit();
            }
            (Key::Char('r'), false, false, _) => {
                if self.state.select_key() {
                    self.begin_edit();
                } else {
                    self.message = Some("only object entries have a key to edit".to_string());
                }
            }
            (Key::Char('a'), false, false, _) => {
                if let Err(err) = self.state.add_entry() {
                    self.message = Some(err.to_string());
                } else {
                    // New properties are named first; array elements have no
                    // key, so `select_key` leaves their value selected.
                    self.state.select_key();
                    self.begin_edit();
                }
            }
            (Key::Char('d') | Key::Char('x') | Key::Delete, false, false, _) => {
                let result = self.state.delete_entry();
                self.report(result);
            }
            (Key::Char('J'), false, false, _) => {
                let result = self.state.move_entry_down();
                self.report(result);
            }
            (Key::Char('K'), false, false, _) => {
                let result = self.state.move_entry_up();
                self.report(result);
            }
            (Key::PageDown, false, false, _) => {
                let top = self.state.scroll() + self.view_height / 2;
                self.state.set_scroll(top);
            }
            (Key::PageUp, false, false, _) => {
                let top = self.state.scroll().saturating_sub(self.view_height / 2);
                self.state.set_scroll(top);
            }
            // Type-to-edit, like Excel: any other character starts editing with
            // an empty text area and is typed into it.
            (Key::Char(_), false, false, _) => {
                self.begin_edit_fresh();
                self.textarea.input(input);
            }
            _ => return false,
        }
        // Demo policy: with `ScrollMode::Manual` the application decides when
        // to follow the cursor.
        self.state.ensure_cursor_visible(self.view_height);
        false
    }

    fn edit_input(&mut self, input: Input) {
        self.message = None;
        match (input.key, input.ctrl, input.alt, input.shift) {
            (Key::Esc, ..) => self.cancel(),
            (Key::Enter | Key::Char('\n' | '\r'), false, false, false) => {
                self.commit_and_move(JsonEditorState::select_down);
            }
            (Key::Enter | Key::Char('\n' | '\r'), false, false, true) => {
                self.commit_and_move(JsonEditorState::select_up);
            }
            (Key::Tab, false, false, false) => {
                self.commit_and_move(JsonEditorState::select_left);
            }
            (Key::Tab, false, false, true) => {
                self.commit_and_move(JsonEditorState::select_right);
            }
            (Key::Enter | Key::Char('\n' | '\r'), ..) => self.textarea.insert_newline(),
            _ => {
                self.textarea.input(input);
            }
        }
    }

    /// Like moving between cells in a spreadsheet: commit what is typed, then
    /// move the selection. An invalid commit keeps the buffer open instead.
    fn commit_and_move(&mut self, move_selection: fn(&mut JsonEditorState) -> bool) {
        if let Err(err) = self.commit_edit() {
            self.message = Some(err.to_string());
            return;
        }
        move_selection(&mut self.state);
        self.state.ensure_cursor_visible(self.view_height);
        self.mode = Mode::Normal;
    }

    fn report(&mut self, result: Result<(), EditError>) {
        if let Err(err) = result {
            self.message = Some(err.to_string());
        }
    }

    /// Starts editing what is selected with the existing text selected, like
    /// Excel's F2: typing replaces it and the cursor sits at the end.
    fn begin_edit(&mut self) {
        self.message = None;
        let text = self.state.edit();
        self.textarea = TextArea::from(text.split('\n'));
        self.textarea.set_tab_length(TAB_LEN as u8);
        self.textarea.select_all();
        self.edit_row = 0;
        self.edit_col = 0;
        self.mode = Mode::Edit;
    }

    /// Starts editing with an empty text area, like typing over a cell in
    /// Excel: the typed text replaces the selected field.
    fn begin_edit_fresh(&mut self) {
        self.begin_edit();
        self.textarea = TextArea::default();
        self.textarea.set_tab_length(TAB_LEN as u8);
    }

    /// Commits the buffer to the selected field; the other is untouched.
    fn commit_edit(&mut self) -> Result<(), EditError> {
        let text = self.buffer_text();
        self.state.commit(&text)
    }

    fn buffer_text(&self) -> String {
        self.textarea.lines().join("\n")
    }

    fn cancel(&mut self) {
        self.mode = Mode::Normal;
        self.message = None;
    }

    /// Mouse: the scrollbar owns its area (clicks, arrows, thumb drags);
    /// clicks select in the tree (key vs value by position) and in the input
    /// line (which places the text cursor); the wheel scrolls the tree.
    fn handle_mouse(&mut self, mouse: MouseEvent) {
        let scrollbar = scrollbar(
            self.state.line_count(),
            self.tree_rect.height as usize,
            self.state.scroll(),
        );
        if let Some(event) = scroll_event(mouse)
            && let Some(ScrollCommand::SetOffset(offset)) =
                scrollbar.handle_event(self.scrollbar_rect, event, &mut self.scrollbar_interaction)
        {
            self.state.set_scroll(offset);
            return;
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if inside(self.tree_rect, mouse) {
                    let row = self.state.scroll() + (mouse.row - self.tree_rect.y) as usize;
                    let col = (mouse.column - self.tree_rect.x) as usize;
                    if self.state.select_at(row, col) {
                        self.state.ensure_cursor_visible(self.view_height);
                    }
                } else if inside(self.input_rect, mouse) {
                    self.place_text_cursor(mouse);
                }
            }
            MouseEventKind::ScrollDown if inside(self.tree_rect, mouse) => {
                let top = self.state.scroll() + 3;
                self.state.set_scroll(top);
            }
            MouseEventKind::ScrollUp if inside(self.tree_rect, mouse) => {
                let top = self.state.scroll().saturating_sub(3);
                self.state.set_scroll(top);
            }
            _ => {}
        }
    }

    /// Clicking the input line starts editing and puts the text cursor under
    /// the pointer.
    fn place_text_cursor(&mut self, mouse: MouseEvent) {
        if self.mode != Mode::Edit {
            self.begin_edit();
        }
        let row = (self.edit_row + (mouse.row - self.input_rect.y) as usize)
            .min(self.textarea.lines().len() - 1);
        let col = (mouse.column - self.input_rect.x) as usize;
        let line = self.textarea.lines()[row].clone();
        self.textarea.cancel_selection();
        self.textarea
            .move_cursor(CursorMove::Jump(row as u16, char_at(&line, col) as u16));
    }
}

// -- rendering --------------------------------------------------------------

fn draw(frame: &mut Frame, app: &mut App) {
    let [input, main, status, help] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(5),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    let [editor_area, output_area] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(main);

    render_input_line(frame, app, input);
    render_editor(frame, app, editor_area);
    render_output(app, output_area, frame);
    render_status(app, status, frame);
    render_help(app, help, frame);
}

fn render_editor(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::bordered().title(" JSON editor ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [tree_area, bar_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    app.view_height = tree_area.height as usize;
    app.tree_rect = tree_area;
    app.scrollbar_rect = bar_area;

    let widget = JsonEditor::new()
        .theme(app.theme.clone())
        .scroll_mode(ScrollMode::Manual);
    frame.render_stateful_widget(&widget, tree_area, &mut app.state);

    let scrollbar = scrollbar(
        app.state.line_count(),
        tree_area.height as usize,
        app.state.scroll(),
    );
    frame.render_widget(&scrollbar, bar_area);
}

fn render_output(app: &App, area: Rect, frame: &mut Frame) {
    let block = Block::bordered().title(" Live output (always valid JSON) ");
    let text = highlight_json(&pretty(app.state.root()), &app.theme);
    frame.render_widget(Paragraph::new(text).block(block), area);
}

/// The text input: a framed full-width box above both panels, one text row
/// tall. It is always visible so the layout never shifts — while editing it
/// shows the live `ratatui-textarea` buffer, otherwise it mirrors what is
/// selected: the key's text or the value's compact JSON.
fn render_input_line(frame: &mut Frame, app: &mut App, area: Rect) {
    let title = match (app.mode, app.state.selected_field()) {
        (Mode::Edit, Field::Value) => " Edit value as JSON text ",
        (Mode::Edit, Field::Key) => " Edit key as plain text ",
        (Mode::Normal, Field::Value) => " JSON text (value selected) ",
        (Mode::Normal, Field::Key) => " Plain text (key selected) ",
    };
    let block = Block::bordered()
        .title(Line::styled(title, app.theme.popup_title))
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
        let spans = clip_spans(runs_to_spans(&chars, &runs, TAB_LEN), 0, inner.width as usize);
        frame.render_widget(Line::from(spans), inner);
    }
}

/// Renders the `ratatui-textarea` buffer with syntax highlighting. The
/// textarea is the editing engine (its `input()` provides undo/redo, word
/// motions, yank/paste, ...); colors, the cursor and the selection are painted
/// with the library's span helpers.
fn render_input(frame: &mut Frame, app: &mut App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let height = area.height as usize;
    let width = area.width as usize;
    if app.mode != Mode::Edit {
        return;
    }
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
        let Some(line) = lines.get(top + row) else { break };
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
            let start = if line_index == start_row { start_col } else { 0 };
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

fn display_width(text: &str) -> usize {
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

fn scrollbar(content_len: usize, viewport_len: usize, offset: usize) -> ScrollBar {
    ScrollBar::vertical(ScrollLengths {
        content_len,
        viewport_len,
    })
    .offset(offset)
    .arrows(ScrollBarArrows::Both)
    .glyph_set(GlyphSet::box_drawing())
}

/// Converts a crossterm mouse event into the scrollbar's backend-agnostic
/// input, so no crossterm feature of `tui-scrollbar` is needed.
fn scroll_event(mouse: MouseEvent) -> Option<ScrollEvent> {
    let pointer = |kind| {
        ScrollEvent::Pointer(PointerEvent {
            column: mouse.column,
            row: mouse.row,
            kind,
            button: PointerButton::Primary,
        })
    };
    let event = match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => pointer(PointerEventKind::Down),
        MouseEventKind::Drag(MouseButton::Left) => pointer(PointerEventKind::Drag),
        MouseEventKind::Up(MouseButton::Left) => pointer(PointerEventKind::Up),
        MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
            let delta = if mouse.kind == MouseEventKind::ScrollDown {
                1
            } else {
                -1
            };
            ScrollEvent::ScrollWheel(ScrollWheel {
                axis: ScrollAxis::Vertical,
                delta,
                column: mouse.column,
                row: mouse.row,
            })
        }
        _ => return None,
    };
    Some(event)
}

fn inside(rect: Rect, mouse: MouseEvent) -> bool {
    mouse.column >= rect.x
        && mouse.column < rect.x + rect.width
        && mouse.row >= rect.y
        && mouse.row < rect.y + rect.height
}

/// The character index at display column `col` of `text`.
fn char_at(text: &str, col: usize) -> usize {
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

fn render_status(app: &App, area: Rect, frame: &mut Frame) {
    let (mode, color) = match (app.mode, app.state.selected_field()) {
        (Mode::Normal, _) => (" NORMAL ", Color::LightCyan),
        (Mode::Edit, Field::Value) => (" EDIT VALUE ", Color::LightYellow),
        (Mode::Edit, Field::Key) => (" EDIT KEY ", Color::LightYellow),
    };
    let mut spans = vec![
        Span::styled(mode, Style::new().fg(Color::Black).bg(color).bold()),
        Span::raw(format!(" {} ", app.state.path_string())),
    ];
    if let Some(message) = &app.message {
        spans.push(Span::styled(
            format!(" ⚠ {message} "),
            Style::new().fg(Color::LightRed),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_help(app: &App, area: Rect, frame: &mut Frame) {
    let text = match (app.mode, app.state.selected_field()) {
        (Mode::Normal, _) => {
            " type to edit  F2 edit  Enter/Tab move  e value  r key  a add  d delete  J/K reorder  PgUp/PgDn  q quit "
        }
        (Mode::Edit, Field::Value) => {
            " typing JSON text (textarea ops: C-u undo, C-w del word, ...)  Enter commit+down  Tab commit+left  Shift goes back  Esc cancel "
        }
        (Mode::Edit, Field::Key) => {
            " typing plain key text (textarea ops: C-u undo, C-w del word, ...)  Enter commit+down  Tab commit+left  Shift goes back  Esc cancel "
        }
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            text,
            Style::new().fg(Color::DarkGray),
        ))),
        area,
    );
}

// -- formatting (the consumer's job: the library never serializes) ----------

/// The demo's pretty printer: two-space indent, `": "` and `", "`. The
/// library deliberately has no opinion here — `Json` is a plain enum to walk
/// and format however you like.
fn pretty(value: &Json) -> String {
    let mut out = String::new();
    write_pretty(value, 0, &mut out);
    out
}

fn write_pretty(value: &Json, level: usize, out: &mut String) {
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Json::Number(n) => out.push_str(n.as_str()),
        Json::String(s) => out.push_str(&quote_string(s)),
        Json::Array(items) if items.is_empty() => out.push_str("[]"),
        Json::Object(entries) if entries.is_empty() => out.push_str("{}"),
        Json::Array(items) => {
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(",\n");
                }
                indent(out, level + 1);
                write_pretty(item, level + 1, out);
            }
            out.push('\n');
            indent(out, level);
            out.push(']');
        }
        Json::Object(entries) => {
            out.push_str("{\n");
            for (i, (key, value)) in entries.iter().enumerate() {
                if i > 0 {
                    out.push_str(",\n");
                }
                indent(out, level + 1);
                out.push_str(&quote_string(key));
                out.push_str(": ");
                write_pretty(value, level + 1, out);
            }
            out.push('\n');
            indent(out, level);
            out.push('}');
        }
    }
}

/// Compact one-liner for the mirror line — the library's compact writer is
/// public; the pretty printer above is intentionally the consumer's own.
fn compact(value: &Json) -> String {
    let mut out = String::new();
    value.write_compact(&mut out);
    out
}

fn indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}
