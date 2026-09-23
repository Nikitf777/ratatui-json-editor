//! A JSON editor widget for [ratatui] that **always produces valid JSON**.
//!
//! The editor works in two modes:
//!
//! * **Normal mode** navigates the JSON tree and applies structural operations
//!   (add, delete, reorder, rename) that cannot break the document.
//! * **Edit mode** types into a [`ratatui_textarea::TextArea`], giving you its
//!   essential text operations (word motions, undo/redo, yank/paste, ...). The
//!   typed text is committed to the document only when it parses as JSON, so
//!   the model is never invalid.
//!
//! Rendering is syntax highlighted with a customizable [`Theme`].
//!
//! ```
//! use ratatui_json_editor::{EditTarget, Input, JsonEditor, Key, Mode, Outcome};
//!
//! let press = |key| Input { key, ..Default::default() };
//! let mut editor = JsonEditor::parse(r#"{"answer": 0}"#).unwrap();
//!
//! // Normal mode: select the first entry and edit its value.
//! editor.handle_input(press(Key::Char('j')));
//! editor.handle_input(press(Key::Char('e')));
//! assert_eq!(editor.mode(), Mode::Edit(EditTarget::Value));
//!
//! // Edit mode: replace the buffer with new JSON text and commit it.
//! editor.handle_input(press(Key::Delete));
//! for c in "\"forty two\"".chars() {
//!     editor.handle_input(press(Key::Char(c)));
//! }
//! assert_eq!(editor.handle_input(press(Key::Enter)), Outcome::Modified);
//! assert_eq!(editor.to_json(), r#"{"answer":"forty two"}"#);
//! ```
//!
//! Run the interactive demo with `cargo run --example demo`.
//!
//! [ratatui]: https://docs.rs/ratatui

mod editor;
mod highlight;
mod json;

pub use editor::{EditTarget, JsonEditor, Mode, Outcome};
pub use highlight::{highlight_json, lex_line, styled_runs, Run, Theme, Token, TokenKind};
pub use json::{quote_string, Json, Number, ParseError};
pub use ratatui_textarea::{Input, Key};
