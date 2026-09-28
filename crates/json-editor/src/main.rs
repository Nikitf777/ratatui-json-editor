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
//! {
//!   "autosave": true,
//!   "binds": {
//!     "delete": [ { "key": "x" } ],
//!     "save":   [ { "key": "s", "ctrl": true }, { "key": "S", "shift": true } ]
//!   }
//! }
//! ```
//!
//! `autosave` (default `false`) saves the file automatically on exit. With it
//! off, quitting with unsaved changes shows a popup first (`q` quits without
//! saving, `Ctrl+S` saves and quits, `Esc` cancels).
//!
//! `binds` moves actions to other keys. Each entry names an action and the
//! list of keys to run it on, so one action can have several. `ctrl`, `alt`
//! and `shift` are false unless the entry says otherwise, so a rebound action
//! never inherits the modifiers of its default — `[{"key": "w"}]` for save is
//! a plain `w`, not `Ctrl+W`. A terminal sends `J`, `K` and `+` with Shift
//! held, so a binding using one needs `"shift": true`. An action with no entry
//! keeps its default. Names: `save`, `quit`, `add`, `duplicate`, `delete`,
//! `move_up`, `move_down`, `move_across_up`, `move_across_down`, `edit_value`,
//! `edit_key`, `hide_block`, `show_block`, `toggle_block`,
//! `select_up`, `select_down`, `select_left`, `select_right`, `select_line_up`,
//! `select_line_down`, `select_word_left`, `select_word_right`,
//! `select_first`, `select_last`, `page_up`, `page_down`, `edit_field` and
//! `menu`. A key is a single character, or one of `esc`, `enter`, `tab`,
//! `space`, `backspace`, `delete`, `home`, `end`, `page_up`, `page_down`,
//! `up`, `down`, `left`, `right`, `f1` through `f12`.
//!
//! A menu bar sits on top (`F10` opens it: File, Edit and View), then two
//! panels:
//! the text input and the JSON tree below.
//!
//! Keys in normal mode:
//!
//! ```text
//! type a char  replace the text and edit     F2          edit with text selected
//! Enter / Shift+Enter  select down / up     Tab / Shift+Tab  select left / right
//! j/k or ↓/↑   select down / up             h/l or ←/→  select left / right
//! e / r        edit value / edit key        a           add entry (key first)
//! d or x       delete entry                 Ctrl+D      duplicate entry
//! H / L        move across the line above / below (into another object)
//! J / K        reorder among siblings        Ctrl+S      save
//! - / + / *    hide / show / toggle the block under the cursor
//! PgUp/PgDn    scroll                       F10         menu bar (File / Edit)
//! q / Esc      quit (saving the result)
//! ```
//!
//! Keys in edit mode: typing through `ratatui-textarea` (undo/redo, word
//! motions, yank/paste, ...), `Enter` / `Shift+Enter` to commit and select
//! down / up, `Tab` / `Shift+Tab` to commit and select left / right, and `Esc`
//! to cancel. Invalid text is never committed.
//!
//! Mouse: click the menu bar to open its buttons, click in the tree to select
//! the key or value under the pointer — a hidden block opens on a second
//! click, once it is selected — click the input line to place the text cursor, wheel to scroll, and
//! the scrollbars handle clicks, arrows and thumb drags.

mod app;
mod config;
mod format;
mod keymap;
mod menu;
mod ui;

use std::fs::{self, OpenOptions};
use std::io::{self, IsTerminal};
use std::path::PathBuf;

use clap::{ArgAction, Parser};
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyEventKind,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::Terminal;
use ratatui_json_editor::{Json, JsonEditorState};
use ratatui_textarea::Input;

use crate::app::{App, Mode};
use crate::config::load_config;
use crate::format::pretty;
use crate::ui::draw;

/// The document a missing file (or a blank session) starts from.
const NEW_DOCUMENT: &str = "{}";

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
