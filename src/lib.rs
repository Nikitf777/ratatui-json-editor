//! A JSON editor widget for [ratatui] that **always produces valid JSON** —
//! and nothing else: no key handling, no text input widget, no chrome.
//!
//! The crate is split into two small halves:
//!
//! * [`JsonEditorState`] — the document, the cursor and the editing
//!   operations. Editing is a transaction: [`JsonEditorState::edit`] hands you
//!   the node's key/value text to put into **whatever input widget you like**,
//!   and [`JsonEditorState::commit`] applies it only when it is valid JSON.
//!   Structural operations (add/delete/reorder) keep the document valid by
//!   construction.
//! * [`JsonEditor`] — a stateless [`StatefulWidget`] that renders the state as
//!   a syntax-highlighted JSON tree ([`Theme`]). Overflow is delegated to the
//!   consumer through [`ScrollMode`] and the state's viewport accessors, so
//!   things like scrollbars stay in the application.
//!
//! Text input, modals, keymaps and scrollbars all live in the consumer. The
//! bundled demo (`cargo run --example demo`) shows one complete composition:
//! a normal mode and an edit mode backed by `ratatui-textarea`, a framed
//! full-width input above the panels, and a scrollbar.
//!
//! ```
//! use ratatui_json_editor::{EditedEntry, JsonEditorState};
//!
//! let mut state = JsonEditorState::parse(r#"{"answer": 0}"#).unwrap();
//! state.cursor_down(); // select root["answer"]
//!
//! // Hand `entry` to your input widget; here we just replace the text.
//! let mut entry = state.edit();
//! assert_eq!(entry.value, "0");
//! entry.value = "\"forty two\"".to_string();
//! assert!(state.commit(entry).is_ok());
//! assert_eq!(state.root().to_compact_string(), r#"{"answer":"forty two"}"#);
//!
//! // Invalid text is rejected and never reaches the document.
//! let bad = EditedEntry { key: None, value: "oops".to_string() };
//! assert!(state.commit(bad).is_err());
//! assert_eq!(state.root().to_compact_string(), r#"{"answer":"forty two"}"#);
//! ```
//!
//! [ratatui]: https://docs.rs/ratatui
//! [`StatefulWidget`]: ratatui_core::widgets::StatefulWidget

mod highlight;
mod json;
mod state;
mod tree;
mod widget;

pub use highlight::{
    clip_spans, highlight_json, lex_line, overlay, runs_to_spans, styled_runs, Run, Theme, Token,
    TokenKind,
};
pub use json::{quote_string, Json, Number, ParseError};
pub use state::{EditError, EditedEntry, Field, JsonEditorState};
pub use widget::{JsonEditor, ScrollMode};
