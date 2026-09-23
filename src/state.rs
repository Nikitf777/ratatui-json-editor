//! JSON tree state and editing operations.
//!
//! [`JsonEditorState`] owns the document and the cursor and nothing else: no
//! key handling, no text buffer, no rendering. Text input is delegated to the
//! consumer as a two-step transaction:
//!
//! 1. [`JsonEditorState::edit`] returns the [`EditedEntry`] — the key and
//!    value text of the selected node — to put into whatever input widget the
//!    consumer renders.
//! 2. [`JsonEditorState::commit`] takes the edited text back and applies it
//!    **only if it is valid** (the value must parse as JSON, keys must be
//!    unique and single-line). On [`EditError`] the state is untouched, so the
//!    document always serializes to valid JSON.
//!
//! Structural operations ([`JsonEditorState::add_entry`],
//! [`JsonEditorState::delete_entry`], [`JsonEditorState::move_entry_up`],
//! [`JsonEditorState::move_entry_down`]) maintain the same guarantee by
//! construction.

use crate::json::{quote_string, Json, ParseError};
use crate::tree::flatten;

/// Why an editing operation was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditError {
    /// The edited value text is not valid JSON. The document is unchanged.
    InvalidJson(ParseError),
    /// Another entry of the object already uses this key.
    DuplicateKey(String),
    /// Keys cannot contain line breaks.
    MultiLineKey,
    /// A key was given for a node that has none (the root value or an array
    /// element).
    KeyNotAllowed,
    /// The operation does not apply to the current node; the message explains
    /// why.
    Refused(&'static str),
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditError::InvalidJson(err) => write!(f, "{err}"),
            EditError::DuplicateKey(key) => write!(f, "duplicate key {}", quote_string(key)),
            EditError::MultiLineKey => write!(f, "a key must fit on one line"),
            EditError::KeyNotAllowed => {
                write!(f, "the root value and array elements have no key")
            }
            EditError::Refused(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for EditError {}

/// The text of one node, ready to hand to a text input widget.
///
/// This is a plain draft: edit the strings however you like and hand the struct
/// back to [`JsonEditorState::commit`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditedEntry {
    /// The key of an object entry. `None` means "keep the current key" (and is
    /// required for nodes that have no key: the root value and array elements).
    pub key: Option<String>,
    /// The value as JSON text, e.g. `"hello"`, `42` or `{"a": [1, 2]}`.
    /// Empty text is treated as `null`.
    pub value: String,
}

/// The document, the cursor, and the scroll position of a JSON tree.
///
/// This state is what a [`crate::JsonEditor`] widget renders; it can also be
/// driven head-less (tests, servers) or combined with any input and chrome the
/// consumer prefers.
pub struct JsonEditorState {
    root: Json,
    cursor: Vec<usize>,
    scroll: usize,
}

impl JsonEditorState {
    /// Creates a state for `root`, with the cursor on the root value.
    pub fn new(root: Json) -> Self {
        Self {
            root,
            cursor: Vec::new(),
            scroll: 0,
        }
    }

    /// Creates a state by parsing a JSON document in text form.
    pub fn parse(src: &str) -> Result<Self, ParseError> {
        Ok(Self::new(Json::parse(src)?))
    }

    /// The current document. It is always valid JSON.
    pub fn root(&self) -> &Json {
        &self.root
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

    /// Moves the cursor to the previous node in pre-order. Returns whether it
    /// moved.
    pub fn cursor_up(&mut self) -> bool {
        self.move_cursor(-1)
    }

    /// Moves the cursor to the next node in pre-order. Returns whether it
    /// moved.
    pub fn cursor_down(&mut self) -> bool {
        self.move_cursor(1)
    }

    /// Moves the cursor to the parent node. Returns whether it moved.
    pub fn cursor_to_parent(&mut self) -> bool {
        self.cursor.pop().is_some()
    }

    /// Moves the cursor to the first child of the selected container. Returns
    /// whether it moved (false for scalars and empty containers).
    pub fn cursor_to_first_child(&mut self) -> bool {
        if child_count(self.selected()) > 0 {
            self.cursor.push(0);
            true
        } else {
            false
        }
    }

    /// Adds a `null` entry and selects it: a child when the cursor is on a
    /// container, otherwise a sibling after the cursor.
    pub fn add_entry(&mut self) -> Result<(), EditError> {
        let cursor = self.cursor.clone();
        let mut new_path = None;
        match self.selected() {
            Json::Array(_) | Json::Object(_) => match node_at_mut(&mut self.root, &cursor) {
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
            },
            _ if cursor.is_empty() => {
                return Err(EditError::Refused(
                    "the root is not a container: edit its value instead",
                ));
            }
            _ => match parent_of(&mut self.root, &cursor) {
                Some((Json::Array(items), index)) => {
                    items.insert(index + 1, Json::Null);
                    new_path = Some(child_path(&cursor[..cursor.len() - 1], index + 1));
                }
                Some((Json::Object(entries), index)) => {
                    let key = unique_key(entries);
                    entries.insert(index + 1, (key, Json::Null));
                    new_path = Some(child_path(&cursor[..cursor.len() - 1], index + 1));
                }
                _ => {}
            },
        }
        match new_path {
            Some(path) => {
                self.cursor = path;
                Ok(())
            }
            None => Err(EditError::Refused("cannot add an entry here")),
        }
    }

    /// Deletes the selected entry and moves the cursor to a neighbor. The root
    /// value cannot be deleted (a document must have a root).
    pub fn delete_entry(&mut self) -> Result<(), EditError> {
        if self.cursor.is_empty() {
            return Err(EditError::Refused("cannot delete the root value"));
        }
        let cursor = self.cursor.clone();
        let index = *cursor.last().unwrap();
        let remaining = match parent_of(&mut self.root, &cursor) {
            Some((Json::Array(items), index)) => {
                items.remove(index);
                items.len()
            }
            Some((Json::Object(entries), index)) => {
                entries.remove(index);
                entries.len()
            }
            _ => return Err(EditError::Refused("cannot delete this node")),
        };
        let mut path = cursor[..cursor.len() - 1].to_vec();
        if remaining > 0 {
            path.push(index.min(remaining - 1));
        }
        self.cursor = path;
        Ok(())
    }

    /// Moves the selected entry one position earlier among its siblings.
    pub fn move_entry_up(&mut self) -> Result<(), EditError> {
        self.move_entry(false)
    }

    /// Moves the selected entry one position later among its siblings.
    pub fn move_entry_down(&mut self) -> Result<(), EditError> {
        self.move_entry(true)
    }

    fn move_entry(&mut self, down: bool) -> Result<(), EditError> {
        if self.cursor.is_empty() {
            return Err(EditError::Refused("cannot reorder the root value"));
        }
        let cursor = self.cursor.clone();
        let index = *cursor.last().unwrap();
        let Some((parent, _)) = parent_of(&mut self.root, &cursor) else {
            return Err(EditError::Refused("cannot reorder this node"));
        };
        let len = child_count(parent);
        let swap_with = if down {
            (index + 1 < len).then_some(index + 1)
        } else {
            index.checked_sub(1)
        };
        let Some(swap_with) = swap_with else {
            return Err(EditError::Refused(if down {
                "already the last entry"
            } else {
                "already the first entry"
            }));
        };
        match parent {
            Json::Array(items) => items.swap(index, swap_with),
            Json::Object(entries) => entries.swap(index, swap_with),
            _ => return Err(EditError::Refused("cannot reorder this node")),
        }
        let mut path = cursor[..cursor.len() - 1].to_vec();
        path.push(swap_with);
        self.cursor = path;
        Ok(())
    }

    /// Returns the editable text of the selected node.
    ///
    /// Nothing is being edited yet — this is a pure draft for the consumer's
    /// input widget. Hand the result (possibly modified) to
    /// [`JsonEditorState::commit`], or drop it to change nothing.
    pub fn edit(&self) -> EditedEntry {
        EditedEntry {
            key: entry_key(&self.root, &self.cursor).map(str::to_string),
            value: self.selected().to_pretty_string(),
        }
    }

    /// Applies an [`EditedEntry`] to the selected node.
    ///
    /// The text is validated first: the value must parse as JSON (empty text
    /// means `null`), keys must be single-line and unique among their siblings.
    /// If anything is wrong, [`EditError`] is returned and the document is
    /// left untouched.
    pub fn commit(&mut self, edited: EditedEntry) -> Result<(), EditError> {
        let EditedEntry { key, value } = edited;
        let value = if value.trim().is_empty() {
            Json::Null
        } else {
            Json::parse(&value).map_err(EditError::InvalidJson)?
        };

        let path = self.cursor.clone();
        if path.is_empty() {
            if key.is_some() {
                return Err(EditError::KeyNotAllowed);
            }
            self.root = value;
            return Ok(());
        }

        let last = *path.last().unwrap();
        let parent_path = &path[..path.len() - 1];
        if let Some(key) = key {
            let Json::Object(entries) = node_at(&self.root, parent_path) else {
                return Err(EditError::KeyNotAllowed);
            };
            if key.contains('\n') {
                return Err(EditError::MultiLineKey);
            }
            if entries
                .iter()
                .enumerate()
                .any(|(i, (other, _))| i != last && *other == key)
            {
                return Err(EditError::DuplicateKey(key));
            }
            if let Some(Json::Object(entries)) = node_at_mut(&mut self.root, parent_path)
                && let Some(entry) = entries.get_mut(last)
            {
                entry.0 = key;
            }
        }
        if let Some(slot) = node_at_mut(&mut self.root, &path) {
            *slot = value;
        }
        Ok(())
    }

    // -- viewport -----------------------------------------------------------

    /// Number of tree rows in the document (including closing rows).
    pub fn line_count(&self) -> usize {
        flatten(&self.root).len()
    }

    /// Index of the row the cursor is on.
    pub fn cursor_line(&self) -> usize {
        flatten(&self.root)
            .iter()
            .position(|row| row.path.as_ref() == Some(&self.cursor))
            .unwrap_or(0)
    }

    /// First visible row (for scrollbars and alike).
    pub fn scroll(&self) -> usize {
        self.scroll
    }

    /// Scrolls so that `top` is the first visible row (clamped to the
    /// document). Pair with [`crate::ScrollMode::Manual`].
    pub fn set_scroll(&mut self, top: usize) {
        self.scroll = top.min(self.line_count().saturating_sub(1));
    }

    /// Scrolls the minimum amount needed to make the cursor row visible in a
    /// viewport of `viewport_height` rows.
    pub fn ensure_cursor_visible(&mut self, viewport_height: usize) {
        let cursor = self.cursor_line();
        if cursor < self.scroll {
            self.scroll = cursor;
        } else if viewport_height > 0 && cursor >= self.scroll + viewport_height {
            self.scroll = cursor + 1 - viewport_height;
        }
    }

    fn move_cursor(&mut self, delta: i32) -> bool {
        let rows = flatten(&self.root);
        let paths: Vec<&Vec<usize>> = rows.iter().filter_map(|row| row.path.as_ref()).collect();
        let pos = paths.iter().position(|p| **p == self.cursor).unwrap_or(0);
        let target = (pos as i32 + delta).clamp(0, paths.len() as i32 - 1) as usize;
        if paths[target] == &self.cursor {
            return false;
        }
        self.cursor = paths[target].clone();
        true
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
    let mut node = root;
    for &index in path {
        node = child_at(node, index).unwrap_or(node);
    }
    node
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
    match node_at(root, parents) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(src: &str) -> JsonEditorState {
        JsonEditorState::parse(src).unwrap()
    }

    fn entry(value: &str) -> EditedEntry {
        EditedEntry {
            key: None,
            value: value.into(),
        }
    }

    fn assert_valid(state: &JsonEditorState) {
        let reparsed = Json::parse(&state.root().to_pretty_string()).unwrap();
        assert_eq!(&reparsed, state.root());
    }

    #[test]
    fn edit_returns_drafts() {
        let state = doc(r#"{"a": [1], "b": null}"#);
        let root = state.edit();
        assert_eq!(root.key, None);
        assert_eq!(root.value, "{\n  \"a\": [\n    1\n  ],\n  \"b\": null\n}");

        let mut state = state;
        state.cursor_down();
        let draft = state.edit();
        assert_eq!(draft.key.as_deref(), Some("a"));
        assert_eq!(draft.value, "[\n  1\n]");

        state.cursor_down();
        assert_eq!(state.edit(), entry("1"));

        state.cursor_down();
        assert_eq!(state.edit().key.as_deref(), Some("b"));
    }

    #[test]
    fn commit_replaces_values_and_converts_types() {
        let mut state = doc(r#"{"a": 1}"#);
        state.cursor_down();
        assert!(state.commit(entry("\"hi\"")).is_ok());
        assert_eq!(state.root(), &Json::parse(r#"{"a": "hi"}"#).unwrap());

        assert!(state.commit(entry("{\"b\": [true, 2.50]}")).is_ok());
        assert_eq!(
            state.root(),
            &Json::parse(r#"{"a": {"b": [true, 2.50]}}"#).unwrap()
        );
        assert_valid(&state);
    }

    #[test]
    fn commit_treats_empty_text_as_null() {
        let mut state = doc(r#"{"a": 1}"#);
        state.cursor_down();
        assert!(state.commit(entry("")).is_ok());
        assert_eq!(state.root(), &Json::parse(r#"{"a": null}"#).unwrap());
    }

    #[test]
    fn invalid_text_never_reaches_the_document() {
        let mut state = doc(r#"{"a": 1}"#);
        state.cursor_down();
        let err = state.commit(entry("{")).unwrap_err();
        assert!(matches!(err, EditError::InvalidJson(_)));
        assert!(err.to_string().contains("line 1"));
        assert_eq!(state.root(), &Json::parse(r#"{"a": 1}"#).unwrap());
        assert_valid(&state);
    }

    #[test]
    fn commit_can_rename_and_checks_keys() {
        let mut state = doc(r#"{"a": 1, "b": 2}"#);
        state.cursor_down();
        let mut draft = state.edit();
        draft.key = Some("c".into());
        assert!(state.commit(draft).is_ok());
        assert_eq!(state.root(), &Json::parse(r#"{"c": 1, "b": 2}"#).unwrap());

        let mut draft = state.edit();
        draft.key = Some("b".into());
        assert_eq!(
            state.commit(draft),
            Err(EditError::DuplicateKey("b".into()))
        );
        let mut draft = state.edit();
        draft.key = Some("x\ny".into());
        assert_eq!(state.commit(draft), Err(EditError::MultiLineKey));
        assert_eq!(state.root(), &Json::parse(r#"{"c": 1, "b": 2}"#).unwrap());
    }

    #[test]
    fn keys_are_only_allowed_on_object_entries() {
        let mut state = doc("[1]");
        state.cursor_down();
        let mut draft = state.edit();
        draft.key = Some("x".into());
        assert_eq!(state.commit(draft), Err(EditError::KeyNotAllowed));

        let mut state = doc(r#"{"a": 1}"#);
        let mut draft = state.edit();
        draft.key = Some("x".into());
        assert_eq!(state.commit(draft), Err(EditError::KeyNotAllowed));
    }

    #[test]
    fn key_none_keeps_the_key() {
        let mut state = doc(r#"{"a": 1}"#);
        state.cursor_down();
        assert!(state.commit(entry("2")).is_ok());
        assert_eq!(state.root(), &Json::parse(r#"{"a": 2}"#).unwrap());
    }

    #[test]
    fn add_delete_and_reorder() {
        let mut state = doc(r#"{"a": []}"#);
        state.cursor_down();
        state.add_entry().unwrap();
        assert_eq!(state.cursor_path(), [0, 0]);
        assert!(state.commit(entry("")).is_ok());
        assert_eq!(state.root(), &Json::parse(r#"{"a": [null]}"#).unwrap());

        let mut state = doc("[1, 2, 3]");
        state.cursor_down();
        state.cursor_down();
        state.move_entry_down().unwrap();
        assert_eq!(state.root(), &Json::parse("[1, 3, 2]").unwrap());
        assert_eq!(state.cursor_path(), [2]);
        state.move_entry_up().unwrap();
        assert_eq!(state.root(), &Json::parse("[1, 2, 3]").unwrap());

        state.delete_entry().unwrap();
        assert_eq!(state.root(), &Json::parse("[1, 3]").unwrap());
        assert_eq!(state.cursor_path(), [1]);
        assert_valid(&state);
    }

    #[test]
    fn refusals_are_explained() {
        let mut state = doc("[1]");
        assert_eq!(
            state.delete_entry(),
            Err(EditError::Refused("cannot delete the root value"))
        );
        assert_eq!(
            state.move_entry_down(),
            Err(EditError::Refused("cannot reorder the root value"))
        );

        let mut state = doc("1");
        assert!(state.add_entry().is_err());

        let mut state = doc("[1]");
        state.cursor_down();
        assert_eq!(
            state.move_entry_down(),
            Err(EditError::Refused("already the last entry"))
        );
    }

    #[test]
    fn navigation_follows_preorder() {
        let mut state = doc(r#"{"a": {"b": [1]}}"#);
        assert_eq!(state.path_string(), "root");
        state.cursor_down();
        assert_eq!(state.path_string(), "root[\"a\"]");
        state.cursor_down();
        assert_eq!(state.path_string(), "root[\"a\"][\"b\"]");
        state.cursor_to_first_child();
        assert_eq!(state.path_string(), "root[\"a\"][\"b\"][0]");
        assert!(!state.cursor_to_first_child(), "scalars have no children");
        state.cursor_to_parent();
        state.cursor_to_parent();
        assert_eq!(state.path_string(), "root[\"a\"]");
        state.cursor_up();
        assert_eq!(state.path_string(), "root");
        assert!(!state.cursor_up(), "already at the top");
    }

    #[test]
    fn viewport_metrics_match_the_tree_layout() {
        let mut state = doc(r#"{"a": [1, 2]}"#);
        assert_eq!(state.line_count(), 6);
        assert_eq!(state.cursor_line(), 0);
        state.cursor_down();
        assert_eq!(state.cursor_line(), 1);
        state.cursor_down();
        assert_eq!(state.cursor_line(), 2);

        state.set_scroll(2);
        assert_eq!(state.scroll(), 2);
        state.set_scroll(999);
        assert!(state.scroll() < state.line_count());

        state.set_scroll(0);
        state.ensure_cursor_visible(2);
        assert_eq!(state.scroll(), 1, "minimal scroll to show the cursor");
        state.ensure_cursor_visible(10);
        assert_eq!(state.scroll(), 1, "already visible");
    }

    #[test]
    fn operations_keep_the_document_valid() {
        let mut state = doc(r#"{"a": [1, 2], "b": null}"#);
        state.cursor_down();
        state.add_entry().unwrap();
        assert_valid(&state);
        state.commit(entry("\"x\"")).unwrap();
        assert_valid(&state);
        state.move_entry_up().unwrap();
        assert_valid(&state);
        state.cursor_to_first_child();
        state.add_entry().unwrap();
        assert_valid(&state);
        state.delete_entry().unwrap();
        assert_valid(&state);
    }
}
