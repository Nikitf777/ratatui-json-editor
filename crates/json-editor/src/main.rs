//! `json-editor` — a terminal JSON editor that always produces valid JSON.
//!
//! ```text
//! json-editor [file.json]      edit a file (saved on Ctrl+S and on exit)
//! cat x.json | json-editor     read stdin, write the result to stdout
//! json-editor --help           show all options (-h; --version / -v)
//! ```
//!
//! With a file argument the document is loaded from that file and saved back
//! to it (`Ctrl+S` saves, and with `autosave` quitting saves too); a missing
//! file starts a new `{}` document and is created when saved. Without one, a
//! piped stdin is read as the document and the result is written to stdout on
//! exit (with a terminal on stdin it starts from `{}`). The TUI always goes to
//! the terminal, so piping stays clean.
//!
//! Input follows the same forgiving rules as editing: empty input is the empty
//! string, bare text is detected as a value (`42`, `true`, `some words`), and
//! text missing only its closing quote or brackets is completed.
//!
//! Settings live in `~/.config/json-editor/config.json`, or in the file given
//! with `--config`:
//!
//! ```json
//! { "autosave": true }
//! ```
//!
//! `autosave` (default `false`) saves the file automatically on exit. With it
//! off, quitting with unsaved changes shows a popup first (`q` quits without
//! saving, `Ctrl+S` saves and quits, `Esc` cancels).
//!
//! Two panels: the text input on top and the JSON tree below.
//!
//! Keys in normal mode:
//!
//! ```text
//! type a char  replace the text and edit     F2          edit with text selected
//! Enter / Shift+Enter  select down / up     Tab / Shift+Tab  select left / right
//! j/k or ↓/↑   select down / up             h/l or ←/→  select left / right
//! e / r        edit value / edit key        a           add entry (key first)
//! d or x       delete entry                 J / K       reorder among siblings
//! PgUp/PgDn    scroll                       Ctrl+S      save
//! q / Esc      quit (saving the result)
//! ```
//!
//! Keys in edit mode: typing through `ratatui-textarea` (undo/redo, word
//! motions, yank/paste, ...), `Enter` / `Shift+Enter` to commit and select
//! down / up, `Tab` / `Shift+Tab` to commit and select left / right, and `Esc`
//! to cancel. Invalid text is never committed.
//!
//! Mouse: click in the tree to select the key or value under the pointer,
//! click the input line to place the text cursor, wheel to scroll, and the
//! scrollbars handle clicks, arrows and thumb drags.

use std::fs::{self, OpenOptions};
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};

use clap::{ArgAction, Parser};

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyEventKind, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Block;
use ratatui::{Frame, Terminal};
use ratatui_json_editor::{
    clip_spans, overlay, quote_string, runs_to_spans, styled_runs, EditError, Field, Json,
    JsonEditor, JsonEditorState, Run, ScrollMode, Theme,
};
use ratatui_textarea::{CursorMove, DataCursor, Input, Key, TextArea};
use tui_popup::Popup;
use tui_scrollbar::{
    GlyphSet, PointerButton, PointerEvent, PointerEventKind, ScrollAxis, ScrollBar, ScrollBarArrows,
    ScrollBarInteraction, ScrollCommand, ScrollEvent, ScrollLengths, ScrollWheel,
};

const TAB_LEN: usize = 2;

/// The document a missing file (or a blank session) starts from.
const NEW_DOCUMENT: &str = "{}";

/// Settings from `~/.config/json-editor/config.json`.
#[derive(Debug, Default)]
struct Config {
    /// Save the file automatically on exit.
    autosave: bool,
}

/// Loads the config from `path`, or from `~/.config/json-editor/config.json`
/// when `path` is `None`. A missing default file means defaults; an explicit
/// path must exist so typos are caught.
fn load_config(path: Option<&Path>) -> io::Result<Config> {
    let (path, explicit) = match path {
        Some(path) => (path.to_path_buf(), true),
        None => {
            let Some(home) = std::env::var_os("HOME") else {
                return Ok(Config::default());
            };
            (
                PathBuf::from(home).join(".config/json-editor/config.json"),
                false,
            )
        }
    };
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound && !explicit => {
            return Ok(Config::default());
        }
        Err(err) => {
            return Err(io::Error::new(
                err.kind(),
                format!("read {}: {err}", path.display()),
            ));
        }
    };
    parse_config(&text)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {err}", path.display())))
}

fn parse_config(text: &str) -> Result<Config, String> {
    let json = Json::parse(text).map_err(|err| err.to_string())?;
    let Json::Object(entries) = &json else {
        return Err("the config must be a JSON object".to_string());
    };
    let mut config = Config::default();
    for (key, value) in entries {
        match (key.as_str(), value) {
            ("autosave", Json::Bool(autosave)) => config.autosave = *autosave,
            ("autosave", _) => return Err("`autosave` must be true or false".to_string()),
            (name, _) => return Err(format!("unknown option {name:?}")),
        }
    }
    Ok(config)
}

/// A terminal JSON editor that always produces valid JSON.
#[derive(Parser)]
#[command(version, disable_version_flag = true)]
struct Cli {
    /// The JSON file to edit (created if missing). Without it, a piped stdin
    /// provides the document and the result is written to stdout.
    file: Option<PathBuf>,
    /// Read settings from this file instead of
    /// `~/.config/json-editor/config.json` (an explicit path must exist).
    #[arg(short = 'c', long = "config")]
    config: Option<PathBuf>,
    /// Print version information.
    #[arg(short = 'v', long = "version", action = ArgAction::Version)]
    version: (),
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();
    let config = load_config(cli.config.as_deref())?;
    let path = cli.file;
    let (source, missing) = match &path {
        Some(path) => match fs::read_to_string(path) {
            Ok(source) => (source, false),
            // A missing file starts a new document; it is created when saved.
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                (NEW_DOCUMENT.to_string(), true)
            }
            Err(err) => {
                return Err(io::Error::new(
                    err.kind(),
                    format!("read {}: {err}", path.display()),
                ));
            }
        },
        None if !io::stdin().is_terminal() => {
            (io::read_to_string(io::stdin())
                .map_err(|err| io::Error::new(err.kind(), format!("read stdin: {err}")))?,
             false)
        }
        None => (NEW_DOCUMENT.to_string(), false),
    };
    // Documents load through the same forgiving rules as editing: empty input
    // is the empty string, bare text is detected as a value, and text missing
    // only its closing quote or brackets is completed. Only text that starts
    // like JSON but stays broken is rejected.
    let mut state = JsonEditorState::new(Json::Null);
    state
        .commit(source.trim())
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;

    let mut app = App::new(state, path, config, missing);
    let result = run(&mut app);
    let output = format!("{}\n", pretty(app.state.root()));
    result?;

    // Files are written by saving (Ctrl+S, the exit autosave, or the popup);
    // without a file the result travels on stdout.
    if app.path.is_none() {
        print!("{output}");
    }
    Ok(())
}

/// The TUI goes to the terminal, never to stdout, so the document can travel
/// through the pipes.
fn run(app: &mut App) -> io::Result<()> {
    let tty = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(|err| io::Error::new(err.kind(), format!("open /dev/tty: {err}")))?;
    let mut terminal = Terminal::new(CrosstermBackend::new(tty.try_clone()?))?;
    enable_raw_mode()?;
    let mut setup = tty.try_clone()?;
    execute!(
        setup,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    let result = event_loop(&mut terminal, app);
    let mut teardown = tty;
    execute!(
        teardown,
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste
    )?;
    disable_raw_mode()?;
    result
}

fn event_loop(terminal: &mut Terminal<CrosstermBackend<fs::File>>, app: &mut App) -> io::Result<()> {
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
    path: Option<PathBuf>,
    config: Config,
    dirty: bool,
    popup: bool,
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
    hbar_rect: Rect,
    scrollbar_interaction: ScrollBarInteraction,
    hbar_interaction: ScrollBarInteraction,
}

impl App {
    fn new(state: JsonEditorState, path: Option<PathBuf>, config: Config, dirty: bool) -> Self {
        Self {
            state,
            path,
            config,
            dirty,
            popup: false,
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
            hbar_rect: Rect::default(),
            scrollbar_interaction: ScrollBarInteraction::new(),
            hbar_interaction: ScrollBarInteraction::new(),
        }
    }

    /// Returns `true` when the application should quit.
    fn handle_key(&mut self, input: Input) -> bool {
        if self.popup {
            return self.popup_key(input);
        }
        match self.mode {
            Mode::Normal => self.normal_key(input),
            Mode::Edit => {
                self.edit_input(input);
                false
            }
        }
    }

    /// The unsaved-changes popup: quit anyway, save and quit, or cancel.
    fn popup_key(&mut self, input: Input) -> bool {
        match input.key {
            Key::Char('q') => true,
            Key::Char('s') => {
                self.save();
                true
            }
            Key::Esc => {
                self.popup = false;
                false
            }
            _ => false,
        }
    }

    /// Quitting: with autosave the file is written on the way out; without it,
    /// unsaved changes ask first.
    fn request_exit(&mut self) -> bool {
        match self.path.as_ref() {
            None => true,
            Some(_) if self.config.autosave => {
                self.save();
                true
            }
            Some(_) if !self.dirty => true,
            Some(_) => {
                self.popup = true;
                false
            }
        }
    }

    fn normal_key(&mut self, input: Input) -> bool {
        self.message = None;
        match (input.key, input.ctrl, input.alt, input.shift) {
            (Key::Char('q') | Key::Esc, false, false, _) | (Key::Char('c'), true, _, _) => {
                return self.request_exit();
            }
            (Key::Char('s'), true, _, _) => {
                self.save();
                return false;
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
                    self.dirty = true;
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
            (Key::Char(_), false, false, _) => {
                self.begin_edit_fresh();
                self.textarea.input(input);
            }
            _ => return false,
        }
        self.state.ensure_cursor_visible(self.view_height);
        self.state.ensure_cursor_visible_x(self.tree_rect.width as usize);
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
        self.dirty = true;
        move_selection(&mut self.state);
        self.state.ensure_cursor_visible(self.view_height);
        self.state.ensure_cursor_visible_x(self.tree_rect.width as usize);
        self.mode = Mode::Normal;
    }

    fn report(&mut self, result: Result<(), EditError>) {
        match result {
            Ok(()) => self.dirty = true,
            Err(err) => self.message = Some(err.to_string()),
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

    /// Writes the document to its file; without a file there is nothing to
    /// save to (the result goes to stdout on exit).
    fn save(&mut self) {
        self.message = match &self.path {
            Some(path) => match fs::write(path, format!("{}\n", pretty(self.state.root()))) {
                Ok(()) => {
                    self.dirty = false;
                    Some("saved".to_string())
                }
                Err(err) => Some(err.to_string()),
            },
            None => Some("no file to save to (the result goes to stdout on exit)".to_string()),
        };
    }

    /// Mouse: the scrollbars own their areas (clicks, arrows, thumb drags);
    /// clicks select in the tree (key vs value by position) and in the input
    /// line (which places the text cursor); the wheel scrolls.
    fn handle_mouse(&mut self, mouse: MouseEvent) {
        if let Some(event) = scroll_event(mouse) {
            let vbar = scrollbar(
                self.state.line_count(),
                self.tree_rect.height as usize,
                self.state.scroll(),
            );
            if let Some(ScrollCommand::SetOffset(offset)) =
                vbar.handle_event(self.scrollbar_rect, event, &mut self.scrollbar_interaction)
            {
                self.state.set_scroll(offset);
                return;
            }
            let hbar = h_scrollbar(
                self.state.content_width(),
                self.tree_rect.width as usize,
                self.state.scroll_x(),
            );
            if let Some(ScrollCommand::SetOffset(offset)) =
                hbar.handle_event(self.hbar_rect, event, &mut self.hbar_interaction)
            {
                self.state.set_scroll_x(offset);
                return;
            }
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if inside(self.tree_rect, mouse) {
                    let row = self.state.scroll() + (mouse.row - self.tree_rect.y) as usize;
                    let col = (mouse.column - self.tree_rect.x) as usize + self.state.scroll_x();
                    if self.state.select_at(row, col) {
                        self.state.ensure_cursor_visible(self.view_height);
                        self.state.ensure_cursor_visible_x(self.tree_rect.width as usize);
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
            MouseEventKind::ScrollDown if inside(self.hbar_rect, mouse) => {
                let x = self.state.scroll_x() + 3;
                self.state.set_scroll_x(x);
            }
            MouseEventKind::ScrollUp if inside(self.hbar_rect, mouse) => {
                let x = self.state.scroll_x().saturating_sub(3);
                self.state.set_scroll_x(x);
            }
            MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight
                if inside(self.tree_rect, mouse) || inside(self.hbar_rect, mouse) =>
            {
                let x = if mouse.kind == MouseEventKind::ScrollRight {
                    self.state.scroll_x() + 3
                } else {
                    self.state.scroll_x().saturating_sub(3)
                };
                self.state.set_scroll_x(x);
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
        let col = (mouse.column - self.input_rect.x) as usize + self.edit_col;
        let line = self.textarea.lines()[row].clone();
        self.textarea.cancel_selection();
        self.textarea
            .move_cursor(CursorMove::Jump(row as u16, char_at(&line, col) as u16));
    }
}

// -- rendering --------------------------------------------------------------

fn draw(frame: &mut Frame, app: &mut App) {
    let [input, editor] = Layout::vertical([Constraint::Length(3), Constraint::Min(3)])
        .areas(frame.area());

    render_input_line(frame, app, input);
    render_editor(frame, app, editor);

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
        let spans = clip_spans(runs_to_spans(&chars, &runs, TAB_LEN), 0, inner.width as usize);
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

// -- helpers ----------------------------------------------------------------

fn scrollbar(content_len: usize, viewport_len: usize, offset: usize) -> ScrollBar {
    ScrollBar::vertical(ScrollLengths {
        content_len,
        viewport_len,
    })
    .offset(offset)
    .arrows(ScrollBarArrows::Both)
    .glyph_set(GlyphSet::box_drawing())
}

fn h_scrollbar(content_len: usize, viewport_len: usize, offset: usize) -> ScrollBar {
    ScrollBar::horizontal(ScrollLengths {
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
        MouseEventKind::ScrollDown
        | MouseEventKind::ScrollUp
        | MouseEventKind::ScrollLeft
        | MouseEventKind::ScrollRight => {
            let (axis, delta) = match mouse.kind {
                MouseEventKind::ScrollRight => (ScrollAxis::Horizontal, 1),
                MouseEventKind::ScrollLeft => (ScrollAxis::Horizontal, -1),
                MouseEventKind::ScrollDown => (ScrollAxis::Vertical, 1),
                _ => (ScrollAxis::Vertical, -1),
            };
            ScrollEvent::ScrollWheel(ScrollWheel {
                axis,
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

// -- formatting (the application's job: the library never pretty-prints) ----

/// The app's pretty printer: two-space indent, `": "` and `", "`.
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
/// public; the pretty printer is the application's own.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_to_autosave_off() {
        assert!(!parse_config("{}").unwrap().autosave);
        assert!(parse_config(r#"{"autosave": true}"#).unwrap().autosave);
        assert!(!parse_config(r#"{"autosave": false}"#).unwrap().autosave);
    }

    #[test]
    fn config_rejects_mistakes() {
        assert!(parse_config(r#"{"autosave": 1}"#).is_err());
        assert!(parse_config(r#"{"autosavee": true}"#).is_err(), "typos are caught");
        assert!(parse_config("[]").is_err(), "the config must be an object");
    }

    #[test]
    fn explicit_config_paths_must_exist() {
        let err = load_config(Some(Path::new("/nonexistent/config.json"))).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(err.to_string().contains("/nonexistent/config.json"));
    }
}
