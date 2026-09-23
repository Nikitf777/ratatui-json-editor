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
//! * a scrollbar, driven by the state's viewport accessors with
//!   `ScrollMode::Manual`.
//!
//! Without an argument the demo starts from a small sample document; with one
//! it loads the given file. On exit the (always valid) JSON is printed to
//! stdout.
//!
//! Keys in normal mode:
//!
//! ```text
//! j/k or ↓/↑   move between nodes        e or Enter   edit value (JSON text)
//! h/l or ←/→   parent / first child      r            rename key
//! a            add entry                 d or x       delete entry
//! J / K        reorder among siblings    PgUp/PgDn    scroll
//! q / Esc      quit
//! ```
//!
//! Keys in edit mode: plain typing through `ratatui-textarea`, so all of its
//! operations work (`Ctrl+U` undo, `Ctrl+R` redo, `Ctrl+W` delete word,
//! `Ctrl+K`/`Ctrl+J` delete to end/start of line, word motions, yank/paste,
//! ...), plus `Enter` to commit (rejected while the text is not valid JSON),
//! `Esc` to cancel and `Tab` to switch between editing the key and the value.

use std::io;

use ratatui::crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};
use ratatui::{DefaultTerminal, Frame};
use ratatui_json_editor::{
    clip_spans, highlight_json, overlay, runs_to_spans, styled_runs, EditedEntry, EditError,
    JsonEditor, JsonEditorState, Run, ScrollMode, Theme,
};
use ratatui_textarea::{DataCursor, Input, Key, TextArea};

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

    println!("{}", app.state.root().to_pretty_string());
    Ok(())
}

fn run(terminal: &mut DefaultTerminal, app: &mut App) -> io::Result<()> {
    execute!(io::stdout(), EnableBracketedPaste)?;
    let result = event_loop(terminal, app);
    execute!(io::stdout(), DisableBracketedPaste)?;
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
            Event::Paste(text) if matches!(app.mode, Mode::Edit(_)) => {
                app.textarea.insert_str(&text);
            }
            _ => {}
        }
    }
}

// -- application state (modes, input, messages) -----------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Key,
    Value,
}

#[derive(Clone)]
struct Form {
    has_key: bool,
    key: String,
    value: String,
    field: Field,
}

enum Mode {
    Normal,
    Edit(Form),
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
        }
    }

    /// Returns `true` when the application should quit.
    fn handle_key(&mut self, input: Input) -> bool {
        match self.mode {
            Mode::Normal => self.normal_key(input),
            Mode::Edit(_) => {
                self.edit_key(input);
                false
            }
        }
    }

    fn normal_key(&mut self, input: Input) -> bool {
        self.message = None;
        match (input.key, input.ctrl) {
            (Key::Char('q') | Key::Esc, false) | (Key::Char('c'), true) => return true,
            (Key::Char('j') | Key::Down, false) => {
                self.state.cursor_down();
            }
            (Key::Char('k') | Key::Up, false) => {
                self.state.cursor_up();
            }
            (Key::Char('h') | Key::Left, false) => {
                self.state.cursor_to_parent();
            }
            (Key::Char('l') | Key::Right, false) => {
                self.state.cursor_to_first_child();
            }
            (Key::Char('e') | Key::Enter, false) => {
                self.begin_edit(Field::Value, false);
            }
            (Key::Char('r'), false) => {
                self.begin_edit(Field::Key, false);
            }
            (Key::Char('a'), false) => {
                if let Err(err) = self.state.add_entry() {
                    self.message = Some(err.to_string());
                } else {
                    self.begin_edit(Field::Value, true);
                }
            }
            (Key::Char('d') | Key::Char('x') | Key::Delete, false) => {
                let result = self.state.delete_entry();
                self.report(result);
            }
            (Key::Char('J'), false) => {
                let result = self.state.move_entry_down();
                self.report(result);
            }
            (Key::Char('K'), false) => {
                let result = self.state.move_entry_up();
                self.report(result);
            }
            (Key::PageDown, false) => {
                let top = self.state.scroll() + self.view_height / 2;
                self.state.set_scroll(top);
            }
            (Key::PageUp, false) => {
                let top = self.state.scroll().saturating_sub(self.view_height / 2);
                self.state.set_scroll(top);
            }
            _ => return false,
        }
        // Demo policy: with `ScrollMode::Manual` the application decides when
        // to follow the cursor.
        self.state.ensure_cursor_visible(self.view_height);
        false
    }

    fn edit_key(&mut self, input: Input) {
        self.message = None;
        let has_key = match &self.mode {
            Mode::Edit(form) => form.has_key,
            Mode::Normal => return,
        };
        match (input.key, input.ctrl, input.alt, input.shift) {
            (Key::Esc, ..) => self.cancel(),
            (Key::Enter | Key::Char('\n' | '\r'), false, false, false) => self.commit(),
            (Key::Enter | Key::Char('\n' | '\r'), ..) => self.textarea.insert_newline(),
            (Key::Tab, ..) if has_key => self.toggle_field(),
            _ => {
                self.textarea.input(input);
            }
        }
    }

    fn report(&mut self, result: Result<(), EditError>) {
        if let Err(err) = result {
            self.message = Some(err.to_string());
        }
    }

    fn begin_edit(&mut self, field: Field, clear_value: bool) {
        self.message = None;
        let entry = self.state.edit();
        let has_key = entry.key.is_some();
        if field == Field::Key && !has_key {
            self.message = Some("only object entries have a key to rename".to_string());
            return;
        }
        let form = Form {
            has_key,
            key: entry.key.unwrap_or_default(),
            value: if clear_value {
                String::new()
            } else {
                entry.value
            },
            field,
        };
        self.load_buffer(&form);
        self.edit_row = 0;
        self.edit_col = 0;
        self.mode = Mode::Edit(form);
    }

    fn load_buffer(&mut self, form: &Form) {
        let text = match form.field {
            Field::Key => &form.key,
            Field::Value => &form.value,
        };
        self.textarea = TextArea::from(text.split('\n'));
        self.textarea.set_tab_length(TAB_LEN as u8);
    }

    fn buffer_text(&self) -> String {
        self.textarea.lines().join("\n")
    }

    fn toggle_field(&mut self) {
        let text = self.buffer_text();
        let Mode::Edit(form) = &self.mode else { return };
        let mut next = form.clone();
        match next.field {
            Field::Key => {
                next.key = text;
                next.field = Field::Value;
            }
            Field::Value => {
                next.value = text;
                next.field = Field::Key;
            }
        }
        self.load_buffer(&next);
        self.edit_row = 0;
        self.edit_col = 0;
        self.mode = Mode::Edit(next);
    }

    fn commit(&mut self) {
        let text = self.buffer_text();
        let Mode::Edit(form) = &mut self.mode else { return };
        match form.field {
            Field::Key => form.key = text,
            Field::Value => form.value = text,
        }
        let Mode::Edit(form) = &self.mode else { return };
        let edited = EditedEntry {
            key: form.has_key.then(|| form.key.clone()),
            value: form.value.clone(),
        };
        match self.state.commit(edited) {
            Ok(()) => {
                self.mode = Mode::Normal;
                self.message = None;
            }
            Err(err) => self.message = Some(err.to_string()),
        }
    }

    fn cancel(&mut self) {
        self.mode = Mode::Normal;
        self.message = None;
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

    let widget = JsonEditor::new()
        .theme(app.theme.clone())
        .scroll_mode(ScrollMode::Manual);
    frame.render_stateful_widget(&widget, tree_area, &mut app.state);

    let mut scrollbar_state = ScrollbarState::new(app.state.line_count())
        .position(app.state.scroll())
        .viewport_content_length(tree_area.height as usize);
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight),
        bar_area,
        &mut scrollbar_state,
    );
}

fn render_output(app: &App, area: Rect, frame: &mut Frame) {
    let block = Block::bordered().title(" Live output (always valid JSON) ");
    let text = highlight_json(&app.state.root().to_pretty_string(), &app.theme);
    frame.render_widget(Paragraph::new(text).block(block), area);
}

/// The text input: a framed full-width box above both panels, one text row
/// tall. It is always visible so the layout never shifts — while editing it
/// shows the live `ratatui-textarea` buffer, otherwise it mirrors the selected
/// node's JSON text (compactly, to fit one line).
fn render_input_line(frame: &mut Frame, app: &mut App, area: Rect) {
    let title = match &app.mode {
        Mode::Edit(form) => match form.field {
            Field::Value => " Edit value as JSON text ",
            Field::Key => " Edit key as plain text ",
        },
        Mode::Normal => " JSON text ",
    };
    let block = Block::bordered()
        .title(Line::styled(title, app.theme.popup_title))
        .border_style(app.theme.popup);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if matches!(app.mode, Mode::Edit(_)) {
        render_input(frame, app, inner);
    } else if inner.height > 0 {
        let text = app.state.selected().to_compact_string();
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
    let Mode::Edit(form) = &app.mode else { return };
    let form = form.clone();
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
        let mut runs: Vec<Run> = match form.field {
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

fn render_status(app: &App, area: Rect, frame: &mut Frame) {
    let (mode, color) = match &app.mode {
        Mode::Normal => (" NORMAL ", Color::LightCyan),
        Mode::Edit(form) => match form.field {
            Field::Value => (" EDIT VALUE ", Color::LightYellow),
            Field::Key => (" EDIT KEY ", Color::LightYellow),
        },
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
    let text = match &app.mode {
        Mode::Normal => {
            " j/k move  h/l parent/child  e edit value  r rename  a add  d delete  J/K reorder  PgUp/PgDn scroll  q quit "
        }
        Mode::Edit(form) => match form.field {
            Field::Value => {
                " typing JSON text (textarea ops: C-u undo, C-w del word, ...)  Enter commit if valid  Esc cancel  Tab key "
            }
            Field::Key => {
                " typing plain key text (textarea ops: C-u undo, C-w del word, ...)  Enter commit  Esc cancel  Tab value "
            }
        },
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            text,
            Style::new().fg(Color::DarkGray),
        ))),
        area,
    );
}
