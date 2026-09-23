//! The JSON tree editor widget.
//!
//! [`JsonEditor`] renders a JSON document as a navigable tree and edits it in
//! two modes:
//!
//! * **Normal mode** — walk the tree and apply structural operations (add,
//!   delete, reorder, rename). These manipulate the [`Json`] model directly and
//!   can never make it invalid.
//! * **Edit mode** — type text through [`ratatui_textarea::TextArea`], which
//!   provides the usual editing operations (word motions, undo/redo, yank and
//!   paste, ...). The buffer holds the JSON text of the value (or the plain key
//!   text) and is committed to the model **only when it parses**. Invalid text
//!   therefore never reaches the document: `editor.root()` always serializes to
//!   valid JSON.

use std::cell::Cell;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Widget};
use ratatui_textarea::{DataCursor, TextArea};

pub use ratatui_textarea::{Input, Key};

use crate::highlight::{clip_spans, overlay, runs_to_spans, styled_runs, Run, Theme};
use crate::json::{self, quote_string, Json};

/// Width of one indentation level in the tree view (and tab size in edit mode).
const TAB_LEN: usize = 2;

/// What the edit popup is editing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditTarget {
    /// The key of an object entry (plain text).
    Key,
    /// The value (raw JSON text).
    Value,
}

/// The mode the editor is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Navigate and apply structural operations.
    Normal,
    /// Text input through the text area ([`EditTarget`] says what is edited).
    Edit(EditTarget),
}

/// What happened to an input event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The key is not used by the editor.
    Ignored,
    /// The key was consumed but the document is unchanged.
    Handled,
    /// The key changed the document.
    Modified,
    /// The key requested leaving the editor (e.g. `q` or `Esc` in normal mode).
    Exit,
}

#[derive(Clone, Debug)]
struct EditSession {
    path: Vec<usize>,
    target: EditTarget,
    key_text: String,
    value_text: String,
}

struct TreeLine {
    spans: Vec<Span<'static>>,
    path: Option<Vec<usize>>,
}

/// A JSON editor widget that always produces valid JSON.
///
/// Feed it with [`JsonEditor::handle_input`] (and optionally
/// [`JsonEditor::handle_paste`]) and render it as any other ratatui widget.
pub struct JsonEditor {
    root: Json,
    cursor: Vec<usize>,
    mode: Mode,
    textarea: TextArea<'static>,
    edit: Option<EditSession>,
    message: Option<String>,
    theme: Theme,
    scroll: Cell<usize>,
    edit_row: Cell<usize>,
    edit_col: Cell<usize>,
}

impl JsonEditor {
    /// Creates an editor for `root`, with the cursor on the root value.
    pub fn new(root: Json) -> Self {
        Self {
            root,
            cursor: Vec::new(),
            mode: Mode::Normal,
            textarea: TextArea::default(),
            edit: None,
            message: None,
            theme: Theme::default(),
            scroll: Cell::new(0),
            edit_row: Cell::new(0),
            edit_col: Cell::new(0),
        }
    }

    /// Creates an editor by parsing a JSON document in text form.
    pub fn parse(src: &str) -> Result<Self, json::ParseError> {
        Ok(Self::new(Json::parse(src)?))
    }

    /// The current document. It is always valid JSON.
    pub fn root(&self) -> &Json {
        &self.root
    }

    /// The document serialized compactly (single line).
    pub fn to_json(&self) -> String {
        self.root.to_compact_string()
    }

    /// The document serialized with two-space indentation.
    pub fn to_pretty_json(&self) -> String {
        self.root.to_pretty_string()
    }

    /// The current mode.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Index path of the selected node (empty means the root value).
    pub fn cursor_path(&self) -> &[usize] {
        &self.cursor
    }

    /// The selected node.
    pub fn selected(&self) -> &Json {
        node_at(&self.root, &self.cursor)
    }

    /// The selected node spelled as a JSON path, e.g. `root["users"][0]`.
    pub fn path_string(&self) -> String {
        let mut out = String::from("root");
        let mut node = &self.root;
        for &i in &self.cursor {
            match node {
                Json::Object(entries) => {
                    let Some((key, value)) = entries.get(i) else { break };
                    out.push_str(&format!("[{}]", quote_string(key)));
                    node = value;
                }
                Json::Array(items) => {
                    let Some(value) = items.get(i) else { break };
                    out.push_str(&format!("[{i}]"));
                    node = value;
                }
                _ => break,
            }
        }
        out
    }

    /// The last status message (e.g. why an operation was refused).
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// The colors used to render JSON.
    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// Replaces the colors used to render JSON.
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    /// Handles a key event, returning what happened to it.
    ///
    /// Accepts anything convertible into [`Input`], including crossterm key
    /// events.
    pub fn handle_input(&mut self, input: impl Into<Input>) -> Outcome {
        match self.mode {
            Mode::Normal => self.handle_normal(input.into()),
            Mode::Edit(_) => self.handle_edit(input.into()),
        }
    }

    /// Inserts pasted text at the cursor while in edit mode.
    pub fn handle_paste(&mut self, text: &str) -> Outcome {
        match self.mode {
            Mode::Edit(_) => {
                self.textarea.insert_str(text);
                Outcome::Handled
            }
            Mode::Normal => Outcome::Ignored,
        }
    }

    fn handle_normal(&mut self, input: Input) -> Outcome {
        self.message = None;
        match (input.key, input.ctrl, input.alt) {
            (Key::Char('q') | Key::Esc, false, false) | (Key::Char('c'), true, _) => Outcome::Exit,
            (Key::Char('j') | Key::Down, false, false) => {
                self.move_vertical(1);
                Outcome::Handled
            }
            (Key::Char('k') | Key::Up, false, false) => {
                self.move_vertical(-1);
                Outcome::Handled
            }
            (Key::Char('h') | Key::Left, false, false) => {
                self.cursor.pop();
                Outcome::Handled
            }
            (Key::Char('l') | Key::Right, false, false) => {
                if child_count(self.selected()) > 0 {
                    self.cursor.push(0);
                }
                Outcome::Handled
            }
            (Key::Char('e') | Key::Enter, false, false) => self.op_edit_value(),
            (Key::Char('r'), false, false) => self.op_rename(),
            (Key::Char('a'), false, false) => self.op_add(),
            (Key::Char('d') | Key::Delete | Key::Char('x'), false, false) => self.op_delete(),
            (Key::Char('J'), false, false) => self.op_move(true),
            (Key::Char('K'), false, false) => self.op_move(false),
            _ => Outcome::Ignored,
        }
    }

    fn handle_edit(&mut self, input: Input) -> Outcome {
        self.message = None;
        match (input.key, input.ctrl, input.alt, input.shift) {
            (Key::Esc, ..) => {
                self.cancel_edit();
                Outcome::Handled
            }
            (Key::Enter | Key::Char('\n' | '\r'), false, false, false) => self.commit(),
            (Key::Enter | Key::Char('\n' | '\r'), ..) => {
                self.textarea.insert_newline();
                Outcome::Handled
            }
            (Key::Tab, ..) if self.editing_object_entry() => {
                self.toggle_edit_target();
                Outcome::Handled
            }
            _ => {
                self.textarea.input(input);
                Outcome::Handled
            }
        }
    }

    // -- operations ---------------------------------------------------------

    fn op_edit_value(&mut self) -> Outcome {
        let text = self.selected().to_pretty_string();
        self.begin_edit(EditTarget::Value, text);
        Outcome::Handled
    }

    fn op_rename(&mut self) -> Outcome {
        if self.editing_object_entry() {
            self.begin_edit_key();
            Outcome::Handled
        } else {
            self.note("only object entries have a key to rename");
            Outcome::Handled
        }
    }

    fn op_add(&mut self) -> Outcome {
        let cursor = self.cursor.clone();
        let mut new_path = None;
        if matches!(self.selected(), Json::Array(_) | Json::Object(_)) {
            match node_at_mut(&mut self.root, &cursor) {
                Some(Json::Array(items)) => {
                    items.push(Json::Null);
                    new_path = Some(child_path(&cursor, items.len() - 1));
                }
                Some(Json::Object(entries)) => {
                    let key = unique_key(entries);
                    entries.push((key, Json::Null));
                    new_path = Some(child_path(&cursor, entries.len() - 1));
                }
                _ => {}
            }
        } else if cursor.is_empty() {
            self.note("the root is not a container: edit it with `e` instead");
        } else if let Some((parent, index)) = parent_of(&mut self.root, &cursor) {
            match parent {
                Json::Array(items) => {
                    items.insert(index + 1, Json::Null);
                    new_path = Some(child_path(&cursor[..cursor.len() - 1], index + 1));
                }
                Json::Object(entries) => {
                    let key = unique_key(entries);
                    entries.insert(index + 1, (key, Json::Null));
                    new_path = Some(child_path(&cursor[..cursor.len() - 1], index + 1));
                }
                _ => {}
            }
        }
        let Some(new_path) = new_path else {
            return Outcome::Handled;
        };
        self.cursor = new_path;
        self.begin_edit(EditTarget::Value, String::new());
        Outcome::Modified
    }

    fn op_delete(&mut self) -> Outcome {
        if self.cursor.is_empty() {
            self.note("cannot delete the root value");
            return Outcome::Handled;
        }
        let cursor = self.cursor.clone();
        let index = *cursor.last().unwrap();
        let Some((parent, _)) = parent_of(&mut self.root, &cursor) else {
            return Outcome::Handled;
        };
        let remaining = match parent {
            Json::Array(items) => {
                items.remove(index);
                items.len()
            }
            Json::Object(entries) => {
                entries.remove(index);
                entries.len()
            }
            _ => return Outcome::Handled,
        };
        let mut path = cursor[..cursor.len() - 1].to_vec();
        if remaining > 0 {
            path.push(index.min(remaining - 1));
        }
        self.cursor = path;
        Outcome::Modified
    }

    fn op_move(&mut self, down: bool) -> Outcome {
        if self.cursor.is_empty() {
            self.note("cannot reorder the root value");
            return Outcome::Handled;
        }
        let cursor = self.cursor.clone();
        let index = *cursor.last().unwrap();
        let Some((parent, _)) = parent_of(&mut self.root, &cursor) else {
            return Outcome::Handled;
        };
        let len = child_count(parent);
        let swap_with = if down {
            (index + 1 < len).then_some(index + 1)
        } else {
            index.checked_sub(1)
        };
        let Some(swap_with) = swap_with else {
            self.note(if down {
                "already the last entry"
            } else {
                "already the first entry"
            });
            return Outcome::Handled;
        };
        match parent {
            Json::Array(items) => items.swap(index, swap_with),
            Json::Object(entries) => entries.swap(index, swap_with),
            _ => return Outcome::Handled,
        }
        let mut path = cursor[..cursor.len() - 1].to_vec();
        path.push(swap_with);
        self.cursor = path;
        Outcome::Modified
    }

    // -- edit mode ----------------------------------------------------------

    fn editing_object_entry(&self) -> bool {
        self.edit
            .as_ref()
            .is_some_and(|s| entry_key(&self.root, &s.path).is_some())
            || (self.edit.is_none() && entry_key(&self.root, &self.cursor).is_some())
    }

    fn begin_edit(&mut self, target: EditTarget, value_text: String) {
        let path = self.cursor.clone();
        let key_text = entry_key(&self.root, &path).unwrap_or("").to_string();
        self.edit = Some(EditSession {
            path,
            target,
            key_text,
            value_text,
        });
        self.mode = Mode::Edit(target);
        self.edit_row.set(0);
        self.edit_col.set(0);
        self.load_edit_buffer();
    }

    fn begin_edit_key(&mut self) {
        let value_text = self.selected().to_pretty_string();
        self.begin_edit(EditTarget::Key, value_text);
    }

    fn load_edit_buffer(&mut self) {
        let Some(session) = &self.edit else { return };
        let text = match session.target {
            EditTarget::Key => &session.key_text,
            EditTarget::Value => &session.value_text,
        };
        self.textarea = TextArea::from(text.split('\n'));
        self.textarea.set_tab_length(TAB_LEN as u8);
    }

    fn edit_text(&self) -> String {
        self.textarea.lines().join("\n")
    }

    fn toggle_edit_target(&mut self) {
        let typed = self.edit_text();
        let Some(session) = self.edit.as_mut() else {
            return;
        };
        session.target = match session.target {
            EditTarget::Key => {
                session.key_text = typed;
                EditTarget::Value
            }
            EditTarget::Value => {
                session.value_text = typed;
                EditTarget::Key
            }
        };
        self.mode = Mode::Edit(session.target);
        self.edit_row.set(0);
        self.edit_col.set(0);
        self.load_edit_buffer();
    }

    fn cancel_edit(&mut self) {
        self.edit = None;
        self.mode = Mode::Normal;
        self.message = None;
    }

    fn commit(&mut self) -> Outcome {
        let Some(session) = self.edit.clone() else {
            return Outcome::Handled;
        };
        let typed = self.edit_text();
        let (key_text, value_text) = match session.target {
            EditTarget::Key => (typed, session.value_text),
            EditTarget::Value => (session.key_text, typed),
        };

        // The typed text only reaches the model when it is valid JSON; the
        // buffer stays open otherwise so the document cannot become invalid.
        let value = if value_text.trim().is_empty() {
            Json::Null
        } else {
            match Json::parse(&value_text) {
                Ok(value) => value,
                Err(err) => {
                    self.note(err.to_string());
                    return Outcome::Handled;
                }
            }
        };

        let path = session.path.clone();
        if path.is_empty() {
            self.root = value;
        } else {
            let last = *path.last().unwrap();
            let parent_path = &path[..path.len() - 1];
            let mut key_error = None;
            if let Json::Object(entries) = node_at(&self.root, parent_path) {
                if key_text.contains('\n') {
                    key_error = Some("a key must fit on one line".to_string());
                } else if entries
                    .iter()
                    .enumerate()
                    .any(|(i, (key, _))| i != last && *key == key_text)
                {
                    key_error = Some(format!("duplicate key {}", quote_string(&key_text)));
                }
            }
            if let Some(message) = key_error {
                self.note(message);
                return Outcome::Handled;
            }
            if let Some(Json::Object(entries)) = node_at_mut(&mut self.root, parent_path)
                && let Some(entry) = entries.get_mut(last)
            {
                entry.0 = key_text;
            }
            if let Some(slot) = node_at_mut(&mut self.root, &path) {
                *slot = value;
            }
        }
        self.cancel_edit();
        Outcome::Modified
    }

    fn note(&mut self, message: impl Into<String>) {
        self.message = Some(message.into());
    }

    // -- navigation ---------------------------------------------------------

    fn move_vertical(&mut self, delta: i32) {
        let mut paths = Vec::new();
        collect_paths(&self.root, &mut Vec::new(), &mut paths);
        let pos = paths.iter().position(|p| *p == self.cursor).unwrap_or(0);
        let target = (pos as i32 + delta).clamp(0, paths.len() as i32 - 1) as usize;
        self.cursor = paths[target].clone();
    }

    // -- rendering ----------------------------------------------------------

    fn build_tree(&self) -> Vec<TreeLine> {
        let mut out = Vec::new();
        push_node(
            &self.root,
            None,
            &mut Vec::new(),
            0,
            false,
            &self.theme,
            &mut out,
        );
        out
    }

    /// Renders the tree (and the edit popup while editing) into `area`.
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let lines = self.build_tree();
        let cursor_line = lines
            .iter()
            .position(|line| line.path.as_ref() == Some(&self.cursor))
            .unwrap_or(0);
        let height = area.height as usize;
        let mut top = self.scroll.get().min(lines.len().saturating_sub(1));
        if cursor_line < top {
            top = cursor_line;
        }
        if cursor_line >= top + height {
            top = cursor_line + 1 - height;
        }
        self.scroll.set(top);

        for (i, line) in lines.iter().skip(top).take(height).enumerate() {
            let row = Rect::new(area.x, area.y + i as u16, area.width, 1);
            if top + i == cursor_line {
                buf.set_style(row, self.theme.cursor_line);
            }
            buf.set_line(area.x, row.y, &Line::from(line.spans.clone()), area.width);
        }

        if let Mode::Edit(target) = self.mode {
            self.render_edit_popup(area, target, buf);
        }
    }

    fn render_edit_popup(&self, area: Rect, target: EditTarget, buf: &mut Buffer) {
        let width = ((area.width as usize * 3) / 4)
            .max(16)
            .min(area.width as usize) as u16;
        let height = (self.textarea.lines().len() + 2)
            .max(3)
            .min(area.height as usize) as u16;
        let rect = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        let title = match target {
            EditTarget::Value => " Edit value as JSON text ",
            EditTarget::Key => " Edit key as plain text ",
        };
        Clear.render(rect, buf);
        let block = Block::bordered()
            .title(Line::styled(title, self.theme.popup_title))
            .border_style(self.theme.popup);
        let inner = block.inner(rect);
        block.render(rect, buf);
        self.render_edit_buffer(inner, target, buf);
    }

    fn render_edit_buffer(&self, area: Rect, target: EditTarget, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let height = area.height as usize;
        let width = area.width as usize;
        let DataCursor(cursor_row, cursor_col) = self.textarea.cursor();

        let mut top = self.edit_row.get();
        if cursor_row < top {
            top = cursor_row;
        }
        if cursor_row >= top + height {
            top = cursor_row + 1 - height;
        }
        self.edit_row.set(top);

        let lines = self.textarea.lines();
        let cursor_x = display_width(
            &lines
                .get(cursor_row)
                .map(|line| line.chars().take(cursor_col).collect::<String>())
                .unwrap_or_default(),
        );
        let mut left = self.edit_col.get();
        if cursor_x < left {
            left = cursor_x;
        }
        if cursor_x >= left + width {
            left = cursor_x + 1 - width;
        }
        self.edit_col.set(left);

        let selection = self.textarea.selection_range();
        for row in 0..height {
            let Some(line) = lines.get(top + row) else {
                break;
            };
            let chars: Vec<char> = line.chars().collect();
            let mut runs: Vec<Run> = match target {
                EditTarget::Value => styled_runs(line, &self.theme),
                EditTarget::Key => vec![(0, chars.len(), self.theme.key)],
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
                    self.theme.selection,
                );
            }
            let at_end = line_index == cursor_row && cursor_col >= chars.len();
            if line_index == cursor_row && cursor_col < chars.len() {
                runs = overlay(runs, (cursor_col, cursor_col + 1), self.theme.cursor);
            }
            let mut spans = runs_to_spans(&chars, &runs, TAB_LEN);
            if at_end {
                spans.push(Span::styled(" ", self.theme.cursor));
            }
            let spans = clip_spans(spans, left, width);
            let row_rect = Rect::new(area.x, area.y + row as u16, area.width, 1);
            if line_index == cursor_row {
                buf.set_style(row_rect, self.theme.cursor_line);
            }
            buf.set_line(area.x, row_rect.y, &Line::from(spans), area.width);
        }
    }
}

impl Widget for &JsonEditor {
    fn render(self, area: Rect, buf: &mut Buffer) {
        JsonEditor::render(self, area, buf);
    }
}

// -- model helpers ----------------------------------------------------------

fn child_count(node: &Json) -> usize {
    match node {
        Json::Array(items) => items.len(),
        Json::Object(entries) => entries.len(),
        _ => 0,
    }
}

fn child_at(node: &Json, index: usize) -> Option<&Json> {
    match node {
        Json::Array(items) => items.get(index),
        Json::Object(entries) => entries.get(index).map(|(_, value)| value),
        _ => None,
    }
}

fn child_at_mut(node: &mut Json, index: usize) -> Option<&mut Json> {
    match node {
        Json::Array(items) => items.get_mut(index),
        Json::Object(entries) => entries.get_mut(index).map(|(_, value)| value),
        _ => None,
    }
}

fn node_at<'a>(root: &'a Json, path: &[usize]) -> &'a Json {
    node_at_opt(root, path).unwrap_or(root)
}

fn node_at_opt<'a>(root: &'a Json, path: &[usize]) -> Option<&'a Json> {
    let mut node = root;
    for &index in path {
        node = child_at(node, index)?;
    }
    Some(node)
}

fn node_at_mut<'a>(root: &'a mut Json, path: &[usize]) -> Option<&'a mut Json> {
    let mut node = root;
    for &index in path {
        node = child_at_mut(node, index)?;
    }
    Some(node)
}

fn parent_of<'a>(root: &'a mut Json, path: &[usize]) -> Option<(&'a mut Json, usize)> {
    let (&last, parents) = path.split_last()?;
    let mut node = root;
    for &index in parents {
        node = child_at_mut(node, index)?;
    }
    Some((node, last))
}

fn entry_key<'a>(root: &'a Json, path: &[usize]) -> Option<&'a str> {
    let (&last, parents) = path.split_last()?;
    match node_at_opt(root, parents)? {
        Json::Object(entries) => entries.get(last).map(|(key, _)| key.as_str()),
        _ => None,
    }
}

fn child_path(parent: &[usize], index: usize) -> Vec<usize> {
    let mut path = parent.to_vec();
    path.push(index);
    path
}

fn unique_key(entries: &[(String, Json)]) -> String {
    if !entries.iter().any(|(key, _)| key == "new") {
        return "new".to_string();
    }
    (1..)
        .map(|n| format!("new{n}"))
        .find(|candidate| !entries.iter().any(|(key, _)| key == candidate))
        .unwrap()
}

fn collect_paths(node: &Json, path: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
    out.push(path.clone());
    for index in 0..child_count(node) {
        path.push(index);
        collect_paths(
            child_at(node, index).unwrap_or(&Json::Null),
            path,
            out,
        );
        path.pop();
    }
}

// -- tree rendering ---------------------------------------------------------

fn push_node(
    node: &Json,
    key: Option<&str>,
    path: &mut Vec<usize>,
    depth: usize,
    comma: bool,
    theme: &Theme,
    out: &mut Vec<TreeLine>,
) {
    let mut spans = vec![Span::raw("  ".repeat(depth))];
    if let Some(key) = key {
        spans.push(Span::styled(quote_string(key), theme.key));
        spans.push(Span::styled(":", theme.punct));
        spans.push(Span::raw(" "));
    }

    let (open, close) = match node {
        Json::Array(_) => ("[", "]"),
        Json::Object(_) => ("{", "}"),
        _ => ("", ""),
    };
    let is_empty = child_count(node) == 0 && !matches!(node, Json::String(_) | Json::Number(_) | Json::Bool(_) | Json::Null);
    match node {
        Json::Array(_) | Json::Object(_) if is_empty => {
            spans.push(Span::styled(format!("{open}{close}"), theme.punct));
            push_comma(&mut spans, comma, theme);
            out.push(TreeLine {
                spans,
                path: Some(path.clone()),
            });
        }
        Json::Array(items) => {
            spans.push(Span::styled(open, theme.punct));
            out.push(TreeLine {
                spans,
                path: Some(path.clone()),
            });
            for (index, item) in items.iter().enumerate() {
                path.push(index);
                push_node(item, None, path, depth + 1, index + 1 < items.len(), theme, out);
                path.pop();
            }
            push_close(close, depth, comma, theme, out);
        }
        Json::Object(entries) => {
            spans.push(Span::styled(open, theme.punct));
            out.push(TreeLine {
                spans,
                path: Some(path.clone()),
            });
            for (index, (entry_key, value)) in entries.iter().enumerate() {
                path.push(index);
                push_node(
                    value,
                    Some(entry_key),
                    path,
                    depth + 1,
                    index + 1 < entries.len(),
                    theme,
                    out,
                );
                path.pop();
            }
            push_close(close, depth, comma, theme, out);
        }
        scalar => {
            let text = match scalar {
                Json::String(s) => Span::styled(quote_string(s), theme.string),
                Json::Number(n) => Span::styled(n.to_string(), theme.number),
                Json::Bool(b) => Span::styled(b.to_string(), theme.boolean),
                _ => Span::styled("null", theme.null),
            };
            spans.push(text);
            push_comma(&mut spans, comma, theme);
            out.push(TreeLine {
                spans,
                path: Some(path.clone()),
            });
        }
    }
}

fn push_comma(spans: &mut Vec<Span<'static>>, comma: bool, theme: &Theme) {
    if comma {
        spans.push(Span::styled(",", theme.punct));
    }
}

fn push_close(
    close: &'static str,
    depth: usize,
    comma: bool,
    theme: &Theme,
    out: &mut Vec<TreeLine>,
) {
    let mut spans = vec![
        Span::raw("  ".repeat(depth)),
        Span::styled(close, theme.punct),
    ];
    push_comma(&mut spans, comma, theme);
    out.push(TreeLine { spans, path: None });
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

#[cfg(test)]
mod tests {
    use super::*;

    fn key(k: Key) -> Input {
        Input {
            key: k,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }

    fn ch(c: char) -> Input {
        key(Key::Char(c))
    }

    fn type_text(editor: &mut JsonEditor, text: &str) {
        for c in text.chars() {
            editor.handle_input(ch(c));
        }
    }

    fn enter(editor: &mut JsonEditor) -> Outcome {
        editor.handle_input(key(Key::Enter))
    }

    fn editor(src: &str) -> JsonEditor {
        JsonEditor::parse(src).unwrap()
    }

    fn assert_valid(editor: &JsonEditor) {
        let reparsed = Json::parse(&editor.to_pretty_json()).unwrap();
        assert_eq!(&reparsed, editor.root(), "document must serialize and parse back");
    }

    #[test]
    fn add_value_commits_valid_json() {
        let mut editor = editor("{}");
        assert_eq!(editor.handle_input(ch('a')), Outcome::Modified);
        assert_eq!(editor.mode(), Mode::Edit(EditTarget::Value));
        type_text(&mut editor, "\"hi\"");
        assert_eq!(enter(&mut editor), Outcome::Modified);
        assert_eq!(editor.mode(), Mode::Normal);
        assert_eq!(editor.root(), &Json::parse(r#"{"new": "hi"}"#).unwrap());
        assert_valid(&editor);
    }

    #[test]
    fn invalid_text_never_reaches_the_document() {
        let mut editor = editor(r#"{"a": 1}"#);
        editor.handle_input(ch('j'));
        assert_eq!(editor.cursor_path(), [0]);
        editor.handle_input(ch('e'));
        assert_eq!(editor.mode(), Mode::Edit(EditTarget::Value));
        editor.handle_input(key(Key::Delete));
        type_text(&mut editor, "{");
        assert_eq!(enter(&mut editor), Outcome::Handled);
        assert_eq!(editor.mode(), Mode::Edit(EditTarget::Value), "buffer stays open");
        assert!(editor.message().is_some(), "parse error is reported");
        assert_eq!(editor.root(), &Json::parse(r#"{"a": 1}"#).unwrap());
        editor.handle_input(key(Key::Esc));
        assert_eq!(editor.mode(), Mode::Normal);
        assert_valid(&editor);
    }

    #[test]
    fn empty_value_commits_as_null() {
        let mut editor = editor(r#"{"a": 1}"#);
        editor.handle_input(ch('j'));
        editor.handle_input(ch('e'));
        editor.handle_input(key(Key::Delete));
        assert_eq!(enter(&mut editor), Outcome::Modified);
        assert_eq!(editor.root(), &Json::parse(r#"{"a": null}"#).unwrap());
    }

    #[test]
    fn edits_convert_value_types() {
        let mut editor = editor(r#"{"a": 1}"#);
        editor.handle_input(ch('j'));
        editor.handle_input(ch('e'));
        editor.handle_input(key(Key::Delete));
        type_text(&mut editor, "{\"b\": [true, 2.50]}");
        assert_eq!(enter(&mut editor), Outcome::Modified);
        assert_eq!(
            editor.root(),
            &Json::parse(r#"{"a": {"b": [true, 2.50]}}"#).unwrap()
        );
        assert_valid(&editor);
    }

    #[test]
    fn rename_checks_duplicates() {
        let mut editor = editor(r#"{"a": 1, "b": 2}"#);
        editor.handle_input(ch('j'));
        editor.handle_input(ch('r'));
        assert_eq!(editor.mode(), Mode::Edit(EditTarget::Key));
        editor.handle_input(key(Key::Delete));
        type_text(&mut editor, "b");
        assert_eq!(enter(&mut editor), Outcome::Handled);
        assert_eq!(editor.mode(), Mode::Edit(EditTarget::Key));
        assert!(editor.message().unwrap().contains("duplicate"));
        editor.handle_input(key(Key::Backspace));
        type_text(&mut editor, "c");
        assert_eq!(enter(&mut editor), Outcome::Modified);
        assert_eq!(editor.root(), &Json::parse(r#"{"c": 1, "b": 2}"#).unwrap());
    }

    #[test]
    fn tab_switches_between_key_and_value() {
        let mut editor = editor(r#"{"a": 1}"#);
        editor.handle_input(ch('j'));
        editor.handle_input(ch('e'));
        editor.handle_input(key(Key::Tab));
        assert_eq!(editor.mode(), Mode::Edit(EditTarget::Key));
        editor.handle_input(key(Key::Delete));
        type_text(&mut editor, "z");
        editor.handle_input(key(Key::Tab));
        assert_eq!(editor.mode(), Mode::Edit(EditTarget::Value));
        assert_eq!(enter(&mut editor), Outcome::Modified);
        assert_eq!(editor.root(), &Json::parse(r#"{"z": 1}"#).unwrap());
    }

    #[test]
    fn delete_moves_the_cursor_to_a_neighbor() {
        let mut editor = editor(r#"{"a": 1, "b": 2}"#);
        editor.handle_input(ch('j'));
        assert_eq!(editor.handle_input(ch('d')), Outcome::Modified);
        assert_eq!(editor.root(), &Json::parse(r#"{"b": 2}"#).unwrap());
        assert_eq!(editor.cursor_path(), [0]);
        assert_eq!(editor.handle_input(ch('d')), Outcome::Modified);
        assert_eq!(editor.root(), &Json::parse(r#"{}"#).unwrap());
        assert_eq!(editor.cursor_path(), Vec::<usize>::new());
        assert_eq!(editor.handle_input(ch('d')), Outcome::Handled);
        assert!(editor.message().is_some());
        assert_valid(&editor);
    }

    #[test]
    fn reorder_swaps_siblings() {
        let mut editor = editor("[1, 2, 3]");
        editor.handle_input(ch('j'));
        editor.handle_input(ch('j'));
        assert_eq!(editor.cursor_path(), [1]);
        assert_eq!(editor.handle_input(ch('J')), Outcome::Modified);
        assert_eq!(editor.root(), &Json::parse("[1, 3, 2]").unwrap());
        assert_eq!(editor.cursor_path(), [2]);
        assert_eq!(editor.handle_input(ch('K')), Outcome::Modified);
        assert_eq!(editor.root(), &Json::parse("[1, 2, 3]").unwrap());
        editor.handle_input(ch('K'));
        assert_eq!(editor.cursor_path(), [0]);
        assert_eq!(editor.handle_input(ch('K')), Outcome::Handled);
        assert!(editor.message().is_some(), "refused with an explanation");
    }

    #[test]
    fn add_children_and_siblings() {
        let mut ed = editor(r#"{"a": []}"#);
        ed.handle_input(ch('l'));
        assert_eq!(ed.cursor_path(), [0]);
        ed.handle_input(ch('a'));
        assert_eq!(ed.cursor_path(), [0, 0]);
        enter(&mut ed);
        assert_eq!(ed.root(), &Json::parse(r#"{"a": [null]}"#).unwrap());

        let mut ed = editor("[1]");
        ed.handle_input(ch('j'));
        ed.handle_input(ch('a'));
        assert_eq!(ed.cursor_path(), [1]);
        type_text(&mut ed, "\"x\"");
        enter(&mut ed);
        assert_eq!(ed.root(), &Json::parse(r#"[1, "x"]"#).unwrap());

        let mut ed = editor("1");
        ed.handle_input(ch('a'));
        assert_eq!(ed.mode(), Mode::Normal);
        assert!(ed.message().is_some());
    }

    #[test]
    fn commits_work_at_any_entry_index() {
        let mut ed = editor(r#"{"a": 1, "b": 2}"#);
        ed.handle_input(ch('j'));
        ed.handle_input(ch('j'));
        assert_eq!(ed.cursor_path(), [1]);
        ed.handle_input(ch('r'));
        ed.handle_input(key(Key::Delete));
        type_text(&mut ed, "c");
        assert_eq!(enter(&mut ed), Outcome::Modified);
        assert_eq!(ed.root(), &Json::parse(r#"{"a": 1, "c": 2}"#).unwrap());

        ed.handle_input(ch('e'));
        ed.handle_input(key(Key::Delete));
        type_text(&mut ed, "9");
        assert_eq!(enter(&mut ed), Outcome::Modified);
        assert_eq!(ed.root(), &Json::parse(r#"{"a": 1, "c": 9}"#).unwrap());
        assert_valid(&ed);
    }

    #[test]
    fn navigation_follows_preorder_and_paths() {
        let mut editor = editor(r#"{"a": {"b": [1]}}"#);
        assert_eq!(editor.path_string(), "root");
        editor.handle_input(ch('j'));
        assert_eq!(editor.path_string(), "root[\"a\"]");
        editor.handle_input(ch('j'));
        assert_eq!(editor.path_string(), "root[\"a\"][\"b\"]");
        editor.handle_input(ch('l'));
        assert_eq!(editor.path_string(), "root[\"a\"][\"b\"][0]");
        editor.handle_input(ch('h'));
        editor.handle_input(ch('h'));
        assert_eq!(editor.path_string(), "root[\"a\"]");
        editor.handle_input(ch('k'));
        assert_eq!(editor.path_string(), "root");
    }

    #[test]
    fn paste_and_pretty_json() {
        let mut editor = editor("{}");
        editor.handle_input(ch('a'));
        editor.handle_paste("{\"b\": 1}");
        assert_eq!(enter(&mut editor), Outcome::Modified);
        assert_eq!(editor.to_json(), r#"{"new":{"b":1}}"#);
        assert_eq!(
            editor.to_pretty_json(),
            "{\n  \"new\": {\n    \"b\": 1\n  }\n}"
        );
    }

    #[test]
    fn exit_is_requested_from_normal_mode_only() {
        let mut editor = editor("{}");
        assert_eq!(editor.handle_input(ch('q')), Outcome::Exit);
        editor.handle_input(ch('a'));
        assert_eq!(editor.handle_input(ch('q')), Outcome::Handled, "`q` types text in edit mode");
        assert_eq!(editor.mode(), Mode::Edit(EditTarget::Value));
    }

    #[test]
    fn renders_tree_and_edit_popup() {
        let mut editor = editor(r#"{"a": "x"}"#);
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        editor.render(area, &mut buf);
        let text: String = buf.content().iter().map(|cell| cell.symbol()).collect();
        assert!(text.contains("\"a\""), "tree shows keys: {text:?}");

        editor.handle_input(ch('j'));
        editor.handle_input(ch('e'));
        let mut buf = Buffer::empty(area);
        editor.render(area, &mut buf);
        let text: String = buf.content().iter().map(|cell| cell.symbol()).collect();
        assert!(text.contains("Edit value"), "edit popup is shown: {text:?}");
    }

    #[test]
    fn operations_keep_the_document_valid() {
        let mut editor = editor(r#"{"a": [1, 2], "b": null}"#);
        editor.handle_input(ch('j'));
        assert_valid(&editor);
        editor.op_add();
        assert_valid(&editor);
        editor.cancel_edit();
        editor.op_move(true);
        assert_valid(&editor);
        editor.handle_input(ch('l'));
        editor.op_add();
        editor.cancel_edit();
        assert_valid(&editor);
        editor.op_delete();
        assert_valid(&editor);
        editor.handle_input(ch('h'));
        editor.op_delete();
        assert_valid(&editor);
    }
}
