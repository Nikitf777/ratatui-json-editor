//! Interactive demo of the JSON editor widget.
//!
//! ```text
//! cargo run --example demo [file.json]
//! ```
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
//! J / K        reorder among siblings    q / Esc      quit
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
use ratatui::widgets::{Block, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use ratatui_json_editor::{highlight_json, EditTarget, Input, JsonEditor, Mode, Outcome};

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

fn main() -> io::Result<()> {
    let mut editor = match std::env::args().nth(1) {
        Some(path) => {
            let text = std::fs::read_to_string(&path)?;
            JsonEditor::parse(&text)
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?
        }
        None => JsonEditor::parse(SAMPLE).expect("the sample is valid JSON"),
    };

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut editor);
    ratatui::restore();
    result?;

    println!("{}", editor.to_pretty_json());
    Ok(())
}

fn run(terminal: &mut DefaultTerminal, editor: &mut JsonEditor) -> io::Result<()> {
    execute!(io::stdout(), EnableBracketedPaste)?;
    let result = event_loop(terminal, editor);
    execute!(io::stdout(), DisableBracketedPaste)?;
    result
}

fn event_loop(terminal: &mut DefaultTerminal, editor: &mut JsonEditor) -> io::Result<()> {
    loop {
        terminal.draw(|frame| draw(frame, editor))?;
        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if editor.handle_input(Input::from(key)) == Outcome::Exit {
                    return Ok(());
                }
            }
            Event::Paste(text) => {
                editor.handle_paste(&text);
            }
            _ => {}
        }
    }
}

fn draw(frame: &mut Frame, editor: &JsonEditor) {
    let [main, status, help] = Layout::vertical([
        Constraint::Min(5),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    let [editor_area, output_area] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(main);

    render_editor(frame, editor, editor_area);
    render_output(editor, output_area, frame);
    render_status(editor, status, frame);
    render_help(editor, help, frame);
}

fn render_editor(frame: &mut Frame, editor: &JsonEditor, area: Rect) {
    let block = Block::bordered().title(" JSON editor ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(editor, inner);
}

fn render_output(editor: &JsonEditor, area: Rect, frame: &mut Frame) {
    let block = Block::bordered().title(" Live output (always valid JSON) ");
    let text = highlight_json(&editor.to_pretty_json(), editor.theme());
    frame.render_widget(Paragraph::new(text).block(block), area);
}

fn render_status(editor: &JsonEditor, area: Rect, frame: &mut Frame) {
    let (mode, color) = match editor.mode() {
        Mode::Normal => (" NORMAL ", Color::LightCyan),
        Mode::Edit(EditTarget::Value) => (" EDIT VALUE ", Color::LightYellow),
        Mode::Edit(EditTarget::Key) => (" EDIT KEY ", Color::LightYellow),
    };
    let mut spans = vec![
        Span::styled(mode, Style::new().fg(Color::Black).bg(color).bold()),
        Span::raw(format!(" {} ", editor.path_string())),
    ];
    if let Some(message) = editor.message() {
        spans.push(Span::styled(format!(" ⚠ {message} "), Style::new().fg(Color::LightRed)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_help(editor: &JsonEditor, area: Rect, frame: &mut Frame) {
    let text = match editor.mode() {
        Mode::Normal => {
            " j/k move  h/l parent/child  e edit value  r rename  a add  d delete  J/K reorder  q quit "
        }
        Mode::Edit(EditTarget::Value) => {
            " typing JSON text (textarea ops work: C-u undo, C-w del word, ...)  Enter commit if valid  Esc cancel  Tab key "
        }
        Mode::Edit(EditTarget::Key) => {
            " typing plain key text (textarea ops work: C-u undo, C-w del word, ...)  Enter commit  Esc cancel  Tab value "
        }
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(text, Style::new().fg(Color::DarkGray)))),
        area,
    );
}
