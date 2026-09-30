//! JSON tree state and editing operations.
//!
//! [`JsonEditorState`] owns the document, the cursor and the key/value
//! selection and nothing else: no key handling, no text buffer, no rendering.
//! Text input is delegated to the consumer as a two-step transaction through
//! the selection:
//!
//! 1. [`JsonEditorState::edit`] returns the text of the selected field — the
//!    key or the value — to put into whatever input widget the consumer
//!    renders.
//! 2. [`JsonEditorState::commit`] applies text back to that field, and **only
//!    if it is valid** (values follow forgiving JSON rules, keys must be
//!    unique and single-line). On [`EditError`] the state is untouched, so the
//!    document always serializes to valid JSON. Keys and values are validated
//!    independently, so committing one never touches the other.
//!
//! Structural operations ([`JsonEditorState::add_entry`],
//! [`JsonEditorState::delete_entry`], [`JsonEditorState::move_entry_up`],
//! [`JsonEditorState::move_entry_down`]) maintain the same guarantee by
//! construction.

use crate::json::{Json, ParseError, quote_string};
use crate::tree::{Row, RowContent, field_spans, flatten, row_width};

/// Why an editing operation was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditError {
    /// The edited value text is not valid JSON. The document is unchanged.
    InvalidJson(ParseError),
    /// Another entry of the object already uses this key.
    DuplicateKey(String),
    /// Keys cannot contain line breaks.
    MultiLineKey,
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
            EditError::Refused(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for EditError {}

/// Which part of a node's line is selected: its key or its value.
///
/// Every line has a value; only object entries have a key. The current
/// selection is reported by [`JsonEditorState::selected_field`] and moved with
/// [`JsonEditorState::select_key`], [`JsonEditorState::select_value`],
/// [`JsonEditorState::select_left`] and [`JsonEditorState::select_right`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// The key of an object entry.
    Key,
    /// The value.
    Value,
}

/// The document, the cursor, and the scroll position of a JSON tree.
///
/// This state is what a [`crate::JsonEditor`] widget renders; it can also be
/// driven head-less (tests, servers) or combined with any input and chrome the
/// consumer prefers.
pub struct JsonEditorState {
    root: Json,
    cursor: Vec<usize>,
    field: Field,
    scroll: usize,
    scroll_x: usize,
    /// Paths of the containers whose blocks are hidden. The document is
    /// untouched: this only changes how the tree is laid out.
    collapsed: Vec<Vec<usize>>,
}

impl JsonEditorState {
    /// Creates a state for `root`, with the cursor on the root value.
    pub fn new(root: Json) -> Self {
        Self {
            root,
            cursor: Vec::new(),
            field: Field::Value,
            scroll: 0,
            scroll_x: 0,
            collapsed: Vec::new(),
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
                    let Some((key, value)) = entries.get(i) else {
                        break;
                    };
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
    pub fn select_up(&mut self) -> bool {
        self.move_cursor(-1)
    }

    /// Moves the cursor to the next node in pre-order. Returns whether it
    /// moved.
    pub fn select_down(&mut self) -> bool {
        self.move_cursor(1)
    }

    /// Moves the cursor to the parent node. Returns whether it moved.
    pub fn cursor_to_parent(&mut self) -> bool {
        let moved = self.cursor.pop().is_some();
        self.clamp_field();
        moved
    }

    /// Moves the cursor to the first child of the selected container. Returns
    /// whether it moved (false for scalars and empty containers).
    pub fn cursor_to_first_child(&mut self) -> bool {
        if child_count(self.selected()) > 0 {
            self.cursor.push(0);
            self.clamp_field();
            true
        } else {
            false
        }
    }

    /// Selects what a pointer hit: the node on tree `row` and the key or value
    /// covering display column `col` of that row.
    ///
    /// Rows are document rows as counted by [`Self::line_count`] — subtract
    /// [`Self::scroll`] from a screen row. The column layout is the rendered
    /// one: two spaces of indent per level, then `"key": value`, so a click on
    /// the key selects the key and anywhere else selects the value. A closing
    /// bracket row selects the block it closes (with its value).
    pub fn select_at(&mut self, row: usize, col: usize) -> bool {
        let rows = flatten(&self.root, &self.collapsed);
        let Some(target) = rows.get(row) else {
            return false;
        };
        let path = target.path.clone();
        self.cursor = path;
        let (key, _) = field_spans(target);
        self.field = match key {
            Some((_, key_end)) if col < key_end => Field::Key,
            _ => Field::Value,
        };
        self.clamp_field();
        true
    }

    /// The key or value currently selected on the cursor line.
    ///
    /// [`Field::Key`] is only ever reported for object entries; every other
    /// node selects [`Field::Value`].
    pub fn selected_field(&self) -> Field {
        self.field
    }

    /// Selects the key of the cursor line. Returns whether the key is now
    /// selected — `false` when the node has no key (the root value or an array
    /// element).
    pub fn select_key(&mut self) -> bool {
        if self.has_key() {
            self.field = Field::Key;
            true
        } else {
            false
        }
    }

    /// Selects the value of the cursor line. Every node has a value, so this
    /// always succeeds and returns `true`.
    pub fn select_value(&mut self) -> bool {
        self.field = Field::Value;
        true
    }

    /// Selects the previous field without ever changing level: the key of a
    /// selected value, or the value of the sibling line above when the key is
    /// selected. Returns `false` at the first field of the level.
    pub fn select_left(&mut self) -> bool {
        match self.field {
            Field::Value if self.has_key() => {
                self.field = Field::Key;
                true
            }
            _ => self.select_sibling_field(false),
        }
    }

    /// Selects the next field without ever changing level: the value of a
    /// selected key, or the key of the sibling line below when the value is
    /// selected. Returns `false` at the last field of the level.
    pub fn select_right(&mut self) -> bool {
        match self.field {
            Field::Key => {
                self.field = Field::Value;
                true
            }
            _ => self.select_sibling_field(true),
        }
    }

    fn select_sibling_field(&mut self, forward: bool) -> bool {
        let Some(&last) = self.cursor.last() else {
            return false;
        };
        let len = child_count(node_at(&self.root, &self.cursor[..self.cursor.len() - 1]));
        let target = if forward {
            (last + 1 < len).then_some(last + 1)
        } else {
            last.checked_sub(1)
        };
        let Some(target) = target else {
            return false;
        };
        *self.cursor.last_mut().unwrap() = target;
        self.field = match (forward, self.has_key()) {
            (true, true) => Field::Key,
            _ => Field::Value,
        };
        true
    }

    fn has_key(&self) -> bool {
        entry_key(&self.root, &self.cursor).is_some()
    }

    fn clamp_field(&mut self) {
        if self.field == Field::Key && !self.has_key() {
            self.field = Field::Value;
        }
    }

    /// Adds an empty string entry and selects it: a child when the cursor is
    /// on a container, otherwise a sibling after the cursor.
    pub fn add_entry(&mut self) -> Result<(), EditError> {
        let cursor = self.cursor.clone();
        let mut new_path = None;
        match self.selected() {
            Json::Array(_) | Json::Object(_) => match node_at_mut(&mut self.root, &cursor) {
                Some(Json::Array(items)) => {
                    items.push(Json::String(String::new()));
                    new_path = Some(child_path(&cursor, items.len() - 1));
                }
                Some(Json::Object(entries)) => {
                    let key = unique_key(entries);
                    entries.push((key, Json::String(String::new())));
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
                    items.insert(index + 1, Json::String(String::new()));
                    new_path = Some(child_path(&cursor[..cursor.len() - 1], index + 1));
                }
                Some((Json::Object(entries), index)) => {
                    let key = unique_key(entries);
                    entries.insert(index + 1, (key, Json::String(String::new())));
                    new_path = Some(child_path(&cursor[..cursor.len() - 1], index + 1));
                }
                _ => {}
            },
        }
        match new_path {
            Some(path) => {
                self.cursor = path;
                self.field = Field::Value;
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
        self.clamp_field();
        Ok(())
    }

    /// Copies the selected entry into a new one right after it, and selects
    /// the copy's key, ready to be renamed. The value is copied as it stands,
    /// so a container comes along whole.
    ///
    /// A property cannot keep the same name, so the copy is given one that is
    /// not taken: `a` becomes `a copy`, and duplicating that gives
    /// `a copy copy`. An array item needs no name and is simply copied. The
    /// root value cannot be duplicated, since a document has only one.
    pub fn duplicate_entry(&mut self) -> Result<(), EditError> {
        if self.cursor.is_empty() {
            return Err(EditError::Refused("cannot duplicate the root value"));
        }
        let cursor = self.cursor.clone();
        let index = *cursor.last().unwrap();
        let slot = index + 1;
        let Some((parent, _)) = parent_of(&mut self.root, &cursor) else {
            return Err(EditError::Refused("cannot duplicate this node"));
        };
        match parent {
            Json::Array(items) => items.insert(slot, items[index].clone()),
            Json::Object(entries) => {
                // The value is cloned before the key is taken, so borrowing
                // both at once does not fight over the vector.
                let (key, value) = entries[index].clone();
                let key = free_key(entries, &key);
                entries.insert(slot, (key, value));
            }
            _ => return Err(EditError::Refused("cannot duplicate this node")),
        }
        self.cursor = child_path(&cursor[..cursor.len() - 1], slot);
        // The copy's name is the part that needs attention, so start there.
        // An array item has no key, and the clamp leaves its value selected.
        self.select_key();
        Ok(())
    }

    /// Removes the selected entry but keeps what was inside it: its children
    /// take its place among its siblings, one level up, so none is lost.
    ///
    /// `{"a": {"x": 1}, "b": 2}` becomes `{"x": 1, "b": 2}` — the opposite of
    /// hiding a block, which only changes what is drawn.
    ///
    /// A node with nothing inside has nothing to move up, so this refuses it —
    /// [`JsonEditorState::delete_entry`] is the one to use there. A child
    /// whose key is already used beside the entry is refused too, as two
    /// entries of an object cannot share a name.
    pub fn unflatten_entry(&mut self) -> Result<(), EditError> {
        if self.cursor.is_empty() {
            return Err(EditError::Refused("cannot delete the root value"));
        }
        let cursor = self.cursor.clone();
        let index = *cursor.last().unwrap();
        let parent_path = &cursor[..cursor.len() - 1];
        let children: Vec<(Option<String>, Json)> = match node_at(&self.root, &cursor) {
            Json::Array(items) => items.iter().cloned().map(|item| (None, item)).collect(),
            Json::Object(entries) => entries
                .iter()
                .map(|(key, value)| (Some(key.clone()), value.clone()))
                .collect(),
            _ => {
                return Err(EditError::Refused("only a container has children to lift"));
            }
        };
        if children.is_empty() {
            return Err(EditError::Refused(
                "only a container with entries has children to lift",
            ));
        }
        // A child cannot step into a place where its key is already taken.
        if let Json::Object(entries) = node_at(&self.root, parent_path) {
            for (key, _) in &children {
                if entries.iter().any(|(other, _)| Some(other) == key.as_ref()) {
                    return Err(EditError::Refused(
                        "a child's key is already used beside this entry",
                    ));
                }
            }
        }
        // The entry leaves, and its children step into the gap it made.
        detach(&mut self.root, &cursor);
        let target = parent_path.to_vec();
        for (at, (key, value)) in (index..).zip(children) {
            match node_at_mut(&mut self.root, &target) {
                // An item lifted out of an array has no name, so it is given
                // one, the same text the input line would show for it.
                Some(Json::Object(entries)) => {
                    let key = key.unwrap_or_else(|| expose_value(&value).trim().to_string());
                    entries.insert(at, (key, value));
                }
                Some(Json::Array(items)) => items.insert(at, value),
                _ => break,
            }
        }
        self.cursor = child_path(&target, index);
        self.clamp_field();
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

    /// Moves the selected entry across the line above it, into whatever that
    /// line holds: a sibling property, the object above, or — for the first
    /// entry of an object — a place beside that object.
    ///
    /// Unlike [`JsonEditorState::move_entry_up`], which only reorders among
    /// siblings, this can change the entry's parent. A property keeps its key,
    /// so it can only be taken in by another object; meeting an array is
    /// refused.
    pub fn move_entry_across_up(&mut self) -> Result<(), EditError> {
        self.move_across(false)
    }

    /// Moves the selected entry across the line below it, into whatever that
    /// line holds: a sibling property, the object below, or — for the last
    /// entry of an object — a place beside that object.
    ///
    /// Unlike [`JsonEditorState::move_entry_down`], which only reorders among
    /// siblings, this can change the entry's parent. A property keeps its key,
    /// so it can only be taken in by another object; meeting an array is
    /// refused.
    pub fn move_entry_across_down(&mut self) -> Result<(), EditError> {
        self.move_across(true)
    }

    /// Moving an entry across a line, which may or may not change its parent.
    ///
    /// The line over in the given direction decides what happens, walking the
    /// rendered lines the same way the cursor walks them and passing over any
    /// container on the way:
    ///
    /// * the nearest property above or below that is a container takes the
    ///   entry in, as its last or first child;
    /// * otherwise a free slot among the siblings is filled, so the move is a
    ///   plain reorder;
    /// * otherwise, for the first or last entry of an object, the entry leaves
    ///   that object and takes its place beside it.
    ///
    /// Array items need no key, so they move through arrays and objects
    /// alike. A property keeps its key, so one can only be taken in by another
    /// object; meeting an array is refused.
    fn move_across(&mut self, down: bool) -> Result<(), EditError> {
        if self.cursor.is_empty() {
            return Err(EditError::Refused("cannot move the root value"));
        }
        let cursor = self.cursor.clone();
        let index = *cursor.last().unwrap();
        let parent_path = &cursor[..cursor.len() - 1];
        let property = matches!(node_at(&self.root, parent_path), Json::Object(_));
        let refuse = || {
            // The document's own object has no line outside it, so its first
            // and last entries are the document's own edges.
            if property && parent_path.is_empty() {
                return EditError::Refused(if down {
                    "already the last entry of the document"
                } else {
                    "already the first entry of the document"
                });
            }
            EditError::Refused(if property {
                if down {
                    "already the last entry of this object, with no property below to move into"
                } else {
                    "already the first entry of this object, with no property above to move into"
                }
            } else if down {
                "already the last entry, with no property below to move into"
            } else {
                "already the first entry, with no property above to move into"
            })
        };
        // A neighbouring property that can hold the entry takes it in, even
        // where a sibling slot is free: moving between objects is what these
        // operations are for.
        if let Some(across) = self.container_across(&cursor, down)
            && across != parent_path
        {
            let count = child_count(node_at(&self.root, &across));
            let slot = if down { 0 } else { count };
            let r = self.transfer(&cursor, &across, slot, &refuse);
            return r;
        }
        // Otherwise a free slot among the siblings is a plain move.
        let len = child_count(node_at(&self.root, parent_path));
        if let Some(slot) = if down {
            (index + 1 < len).then_some(index + 1)
        } else {
            index.checked_sub(1)
        } {
            self.swap_entry(parent_path, index, slot);
            return Ok(());
        }
        // With no neighbour to adopt it, the entry leaves the object it is in
        // and takes its place beside it — but only when that object is a
        // property of another. The document's own object has no line outside
        // it, so its first and last entries are the document's edges.
        if parent_path.is_empty() {
            return Err(refuse());
        }
        let slot = *parent_path.last().unwrap() + usize::from(down);
        let grandparent = parent_path[..parent_path.len() - 1].to_vec();
        self.transfer(&cursor, &grandparent, slot, &refuse)
    }

    /// Moves the entry at `cursor` into the container at `target`, at slot
    /// `index`. The entry leaves the document first, and the slot is worked out
    /// from the tree as it is then — the only shape that can be indexed into
    /// safely.
    fn transfer(
        &mut self,
        cursor: &[usize],
        target: &[usize],
        index: usize,
        refuse: &impl Fn() -> EditError,
    ) -> Result<(), EditError> {
        let property = matches!(
            node_at(&self.root, &cursor[..cursor.len() - 1]),
            Json::Object(_)
        );
        if property && matches!(node_at(&self.root, target), Json::Array(_)) {
            return Err(EditError::Refused(
                "a property keeps its key, so it cannot move into an array",
            ));
        }
        // The place the entry is going must be a container, and checking it
        // before anything moves leaves the document untouched if it is not.
        if !matches!(
            node_at(&self.root, target),
            Json::Array(_) | Json::Object(_)
        ) {
            return Err(refuse());
        }
        let (mut key, value) = detach(&mut self.root, cursor);
        // The removal shifts every index after it among the same siblings, so
        // where the entry lands is worked out on the smaller tree.
        let target = shift_after_removal(target, cursor);
        if !matches!(
            node_at(&self.root, &target),
            Json::Array(_) | Json::Object(_)
        ) {
            return Err(refuse());
        }
        // An array item takes a name from its own text when it lands in an
        // object; a property brings its key along.
        if key.is_none() && matches!(node_at(&self.root, &target), Json::Object(_)) {
            key = Some(expose_value(&value).trim().to_string());
        }
        let index = index.min(child_count(node_at(&self.root, &target)));
        let child = child_path(&target, index);
        attach(&mut self.root, &target, index, key, value);
        self.cursor = child;
        self.clamp_field();
        Ok(())
    }

    /// Swaps two entries of the container at `path` and leaves the cursor on
    /// the one that moved.
    fn swap_entry(&mut self, path: &[usize], from: usize, to: usize) {
        match node_at_mut(&mut self.root, path) {
            Some(Json::Array(items)) => items.swap(from, to),
            Some(Json::Object(entries)) => entries.swap(from, to),
            _ => return,
        }
        let mut cursor = path.to_vec();
        cursor.push(to);
        self.cursor = cursor;
        self.clamp_field();
    }

    /// The nearest property line over in `direction` from the object at
    /// `parent_path` whose value can take an entry. Lines that cannot are
    /// passed over: closing brackets, scalars, and — for an entry with a key
    /// — arrays, which have no keys to keep it in.
    fn container_across(&self, cursor: &[usize], down: bool) -> Option<Vec<usize>> {
        let with_key = matches!(
            node_at(&self.root, &cursor[..cursor.len() - 1]),
            Json::Object(_)
        );
        let parent_path = &cursor[..cursor.len() - 1];
        let lines = flatten(&self.root, &self.collapsed);
        let is_close = |row: &Row| matches!(row.content, RowContent::Close { .. });
        // The line the search starts from. Up steps off the container's own
        // line; down steps past the closing bracket that follows its children.
        // The document's own object is the exception: its entries are rendered
        // inside its line, so the search starts among them, and running past
        // either end means there is nothing left to find.
        let from = if parent_path.is_empty() {
            let index = cursor.last().copied().unwrap_or(0);
            let count = child_count(&self.root);
            if down {
                if index + 1 == count {
                    return None;
                }
                lines
                    .iter()
                    .position(|row| row.path == vec![index + 1] && !is_close(row))?
            } else {
                if index == 0 {
                    return None;
                }
                lines
                    .iter()
                    .position(|row| row.path == vec![index - 1] && !is_close(row))?
            }
        } else {
            let own = lines
                .iter()
                .position(|row| row.path == *parent_path && !is_close(row))?;
            if !down {
                own.checked_sub(1)?
            } else {
                match lines[own + 1..]
                    .iter()
                    .position(|row| row.path == *parent_path && is_close(row))
                {
                    Some(offset) => own + offset + 2,
                    None => own + 1,
                }
            }
        };
        // Lines that cannot hold the entry are passed over: closing brackets,
        // scalars, and — for an entry with a key — arrays.
        let candidates = if down {
            lines.get(from..)?
        } else {
            &lines[..=from]
        };
        let line = if down {
            candidates.iter().find(|row| self.can_hold(row, with_key))?
        } else {
            candidates
                .iter()
                .rev()
                .find(|row| self.can_hold(row, with_key))?
        };
        Some(line.path.clone())
    }

    /// Whether a rendered line is a property whose value can take an entry. A
    /// property keeps its key, so only another object can hold it.
    fn can_hold(&self, row: &Row, with_key: bool) -> bool {
        let has_key = match &row.content {
            RowContent::Container { key, .. } | RowContent::Scalar { key, .. } => key.is_some(),
            RowContent::Close { .. } => false,
        };
        if !has_key {
            return false;
        }
        match node_at(&self.root, &row.path) {
            Json::Object(_) => true,
            Json::Array(_) => !with_key,
            _ => false,
        }
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
        self.clamp_field();
        Ok(())
    }

    /// Hides the selected container's block, so the tree shows it as one line
    /// ending in `[...]` or `{...}`.
    ///
    /// This changes only what is drawn: the document keeps every entry, and
    /// [`JsonEditorState::expand_block`] brings them back. A block that is
    /// already hidden, or that has no entries to hide, is left as it is.
    pub fn collapse_block(&mut self) {
        if self.collapsed.contains(&self.cursor) {
            return;
        }
        if child_count(self.selected()) == 0 {
            return;
        }
        self.collapsed.push(self.cursor.clone());
        self.clamp_scroll();
    }

    /// Shows a hidden block again, as [`JsonEditorState::collapse_block`]
    /// hides it. Expanding a container hides nothing inside it: the blocks
    /// below stay as they were, so a deep tree can be opened a level at a
    /// time.
    pub fn expand_block(&mut self) {
        self.collapsed.retain(|path| *path != self.cursor);
        self.clamp_scroll();
    }

    /// Keeps the view inside the tree after it has grown or shrunk, so the
    /// cursor never ends up off the top of it.
    fn clamp_scroll(&mut self) {
        self.scroll = self.scroll.min(self.line_count().saturating_sub(1));
    }

    /// The line the node at `path` renders on, counted as
    /// [`Self::line_count`] counts them, or `None` when there is no such line.
    /// [`Self::select_at`] is its inverse: it turns a line into a selection.
    pub fn row_of(&self, path: &[usize]) -> Option<usize> {
        flatten(&self.root, &self.collapsed)
            .iter()
            .filter(|row| !matches!(row.content, RowContent::Close { .. }))
            .position(|row| row.path == path)
    }

    /// Whether the selected container's block is hidden.
    pub fn is_collapsed(&self) -> bool {
        self.collapsed.contains(&self.cursor)
    }

    /// The paths of the containers whose blocks are hidden, outermost first.
    pub fn collapsed_paths(&self) -> &[Vec<usize>] {
        &self.collapsed
    }

    /// Turns the selected value into a string holding its JSON text: `42`
    /// becomes `"42"`, `{"a": 1}` becomes `"{\"a\":1}"`.
    ///
    /// A string is left as it is — it is already its own text — so this is a
    /// no-op on one, and says so rather than doing nothing quietly. The
    /// opposite is [`JsonEditorState::parse_value_text`].
    pub fn value_to_string(&mut self) -> Result<(), EditError> {
        if self.field != Field::Value {
            return Err(EditError::Refused(
                "only a value can be turned into a string",
            ));
        }
        if matches!(self.selected(), Json::String(_)) {
            return Err(EditError::Refused("it is already a string"));
        }
        self.replace_value(Json::String(compact(self.selected())));
        Ok(())
    }

    /// Turns a string into the value its text describes, the way
    /// [`JsonEditorState::commit`] reads text: `"42"` becomes `42` and
    /// `"{\"a\":1}"` becomes `{"a": 1}`.
    ///
    /// Text that describes nothing is refused, leaving the string alone. A
    /// bare word is not read as a string here — that would make every string
    /// a number or a boolean by accident — so a plain text string has to be
    /// written as `\"42\"`, quotes and all.
    pub fn parse_value_text(&mut self) -> Result<(), EditError> {
        if self.field != Field::Value {
            return Err(EditError::Refused("only a value can be read as text"));
        }
        let Json::String(text) = self.selected() else {
            return Err(EditError::Refused("it is not a string to read"));
        };
        // A leading quote is what marks the text as JSON rather than a word.
        let trimmed = text.trim();
        let value = if trimmed.starts_with('"') {
            Json::String(text.clone())
        } else {
            interpret_value(trimmed).map_err(EditError::InvalidJson)?
        };
        self.replace_value(value);
        Ok(())
    }

    /// Replaces the selected value, keeping the cursor where it is.
    fn replace_value(&mut self, value: Json) {
        if let Some(node) = node_at_mut(&mut self.root, &self.cursor) {
            *node = value;
        }
    }

    /// Returns the editable text of the selected field: the key's plain text,
    /// or the value in the forgiving form described by
    /// [`JsonEditorState::commit`] — a string's content without quotes, a
    /// container's items without the outer brackets. Hand it to whatever input
    /// widget you like and pass the result back to
    /// [`JsonEditorState::commit`], or drop it to change nothing.
    pub fn edit(&self) -> String {
        match self.field {
            Field::Key => entry_key(&self.root, &self.cursor)
                .unwrap_or_default()
                .to_string(),
            Field::Value => expose_value(self.selected()),
        }
    }

    /// Applies text to the selected field.
    ///
    /// Values are interpreted forgivingly: strict JSON wins (with missing
    /// closers completed), comma-separated items become an array (or an object
    /// when every item is a `"key": value` pair), and any other bare text
    /// becomes a string (so empty text is the empty string, and `"some"` is a
    /// string while `"a", "b"` is an array). Keys must be single-line and
    /// unique among their siblings. Keys and values are validated
    /// independently: committing one never touches the other, and on
    /// [`EditError`] the document is left untouched.
    pub fn commit(&mut self, text: impl AsRef<str>) -> Result<(), EditError> {
        let text = text.as_ref();
        let path = self.cursor.clone();
        if self.field == Field::Value {
            let value = interpret_value(text).map_err(EditError::InvalidJson)?;
            if path.is_empty() {
                self.root = value;
            } else if let Some(slot) = node_at_mut(&mut self.root, &path) {
                *slot = value;
            }
            return Ok(());
        }

        // A key selection implies an object entry.
        let Some((&last, parent_path)) = path.split_last() else {
            return Err(EditError::Refused("the root value has no key"));
        };
        if text.contains('\n') {
            return Err(EditError::MultiLineKey);
        }
        let Json::Object(entries) = node_at(&self.root, parent_path) else {
            return Err(EditError::Refused("only object entries have keys"));
        };
        if entries
            .iter()
            .enumerate()
            .any(|(i, (other, _))| i != last && *other == text)
        {
            return Err(EditError::DuplicateKey(text.to_string()));
        }
        if let Some(Json::Object(entries)) = node_at_mut(&mut self.root, parent_path)
            && let Some(entry) = entries.get_mut(last)
        {
            entry.0 = text.to_string();
        }
        Ok(())
    }

    // -- viewport -----------------------------------------------------------

    /// Number of tree rows in the document (including closing rows).
    pub fn line_count(&self) -> usize {
        flatten(&self.root, &self.collapsed).len()
    }

    /// Index of the row the cursor is on.
    pub fn cursor_line(&self) -> usize {
        flatten(&self.root, &self.collapsed)
            .iter()
            .position(|row| row.path == self.cursor)
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

    /// Widest row in display columns — the content length for horizontal
    /// scrollbars.
    pub fn content_width(&self) -> usize {
        flatten(&self.root, &self.collapsed)
            .iter()
            .map(row_width)
            .max()
            .unwrap_or(0)
    }

    /// Horizontal offset of the first visible display column.
    pub fn scroll_x(&self) -> usize {
        self.scroll_x
    }

    /// Scrolls horizontally so that `x` is the first visible column (clamped
    /// to the content). Pair with [`crate::ScrollMode::Manual`].
    pub fn set_scroll_x(&mut self, x: usize) {
        self.scroll_x = x.min(self.content_width().saturating_sub(1));
    }

    /// Scrolls horizontally the minimum amount needed to show the selected
    /// field's text in a viewport of `viewport_width` columns.
    pub fn ensure_cursor_visible_x(&mut self, viewport_width: usize) {
        let rows = flatten(&self.root, &self.collapsed);
        let Some(row) = rows.iter().find(|row| row.path == self.cursor) else {
            return;
        };
        let (key, value) = field_spans(row);
        let (start, end) = match self.field {
            Field::Key => key.unwrap_or(value),
            Field::Value => value,
        };
        if start < self.scroll_x {
            self.scroll_x = start;
        } else if end - self.scroll_x > viewport_width {
            self.scroll_x = if end - start > viewport_width {
                start
            } else {
                end - viewport_width
            };
        }
    }

    fn move_cursor(&mut self, delta: i32) -> bool {
        let rows = flatten(&self.root, &self.collapsed);
        let paths: Vec<&Vec<usize>> = rows
            .iter()
            .filter(|row| !matches!(row.content, RowContent::Close { .. }))
            .map(|row| &row.path)
            .collect();
        let pos = paths.iter().position(|p| **p == self.cursor).unwrap_or(0);
        let target = (pos as i32 + delta).clamp(0, paths.len() as i32 - 1) as usize;
        if paths[target] == &self.cursor {
            return false;
        }
        self.cursor = paths[target].clone();
        self.clamp_field();
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

/// The object entry at `path` with its key, or `None` when the path is not
/// one — array items and the root have no key.
fn take_entry(node: &mut Json, index: usize) -> Option<(String, Json)> {
    match node {
        Json::Object(entries) => Some(entries.remove(index)),
        _ => None,
    }
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

/// The path to the same place once the entry at `removed` has been taken out
/// of the document: every index that pointed past it in the same parent moves
/// down one.
fn shift_after_removal(path: &[usize], removed: &[usize]) -> Vec<usize> {
    // Only a target that was a sibling of the removed entry moves; anything
    // in another container keeps its path.
    if path.len() != removed.len() || path[..path.len() - 1] != removed[..removed.len() - 1] {
        return path.to_vec();
    }
    let mut shifted = path.to_vec();
    let last = shifted.len() - 1;
    if shifted[last] > removed[last] {
        shifted[last] -= 1;
    }
    shifted
}

/// A key close to `base` that no entry uses: `a`, then `a copy`,
/// `a copy copy`.
fn free_key(entries: &[(String, Json)], base: &str) -> String {
    let taken = |key: &str| entries.iter().any(|(other, _)| other == key);
    if !taken(&format!("{base} copy")) {
        return format!("{base} copy");
    }
    (2..)
        .map(|n| format!("{base} copy {n}"))
        .find(|candidate| !taken(candidate))
        .unwrap()
}

/// Removes the entry at `path` from the document. Object entries hand back
/// their key so it can be carried along; array items have none.
fn detach(root: &mut Json, path: &[usize]) -> (Option<String>, Json) {
    let Some((parent, index)) = parent_of(root, path) else {
        return (None, Json::Null);
    };
    if let Some(entry) = take_entry(parent, index) {
        return (Some(entry.0), entry.1);
    }
    match parent {
        Json::Array(items) => (None, items.remove(index)),
        _ => (None, Json::Null),
    }
}

/// Puts a detached entry into the container at `parent_path` before its
/// entries. A property needs an object to hold its key; the caller has
/// checked that already.
fn attach(root: &mut Json, parent_path: &[usize], index: usize, key: Option<String>, value: Json) {
    match node_at_mut(root, parent_path) {
        Some(Json::Object(entries)) => {
            entries.insert(index, (key.unwrap_or_default(), value));
        }
        Some(Json::Array(items)) => items.insert(index, value),
        _ => {}
    }
}

/// The closing characters missing from `text` when it ends inside a string or
/// container: `"some` needs `"`, `{"a": [1` needs `]}`. `None` when nothing is
/// missing.
fn missing_closers(text: &str) -> Option<String> {
    let mut closers = String::new();
    let mut open = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    for c in text.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
        } else {
            match c {
                '"' => in_string = true,
                '[' => open.push(']'),
                '{' => open.push('}'),
                ']' | '}' => {
                    open.pop();
                }
                _ => {}
            }
        }
    }
    if in_string {
        closers.push('"');
    }
    while let Some(closer) = open.pop() {
        closers.push(closer);
    }
    (!closers.is_empty()).then_some(closers)
}

/// Strict JSON parsing, completing text that is only missing its closing quote
/// or brackets.
fn parse_completing(text: &str) -> Result<Json, ParseError> {
    Json::parse(text).or_else(|err| {
        missing_closers(text)
            .and_then(|closers| Json::parse(&format!("{text}{closers}")).ok())
            .ok_or(err)
    })
}

/// The editable text of a value: see [`JsonEditorState::edit`].
fn expose_value(value: &Json) -> String {
    match value {
        Json::String(s) => s.clone(),
        Json::Number(n) => n.to_string(),
        Json::Bool(b) => b.to_string(),
        Json::Null => "null".to_string(),
        Json::Array(items) if items.is_empty() => "[]".to_string(),
        Json::Object(entries) if entries.is_empty() => "{}".to_string(),
        Json::Array(items) => items.iter().map(compact).collect::<Vec<_>>().join(", "),
        Json::Object(entries) => entries
            .iter()
            .map(|(key, value)| format!("{}: {}", quote_string(key), compact(value)))
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// Compact JSON text of a value, for nested items in [`expose_value`].
fn compact(value: &Json) -> String {
    let mut out = String::new();
    value.write_compact(&mut out);
    out
}

/// Interprets edited value text: see [`JsonEditorState::commit`]. Strict JSON (with
/// missing closers completed) wins; comma-separated items become an array, or
/// an object when every item is a `"key": value` pair; any other bare text
/// becomes a string. Text that starts like JSON but stays broken is an error.
fn interpret_value(text: &str) -> Result<Json, ParseError> {
    let raw_items = split_top_commas(text);
    if raw_items.len() > 1 {
        let items: Vec<&str> = raw_items.iter().map(|item| item.trim()).collect();
        if items.iter().all(|item| split_entry(item).is_some()) {
            let mut entries = Vec::new();
            for item in items {
                let (key, rest) = split_entry(item).unwrap();
                entries.push((key, interpret_value(rest)?));
            }
            return Ok(Json::Object(entries));
        }
        let values = items
            .into_iter()
            .map(interpret_value)
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(Json::Array(values));
    }
    if let Some((key, rest)) = split_entry(text) {
        return Ok(Json::Object(vec![(key, interpret_value(rest)?)]));
    }
    let parsed = parse_completing(text);
    if parsed.is_ok() || text.trim_start().starts_with(['"', '[', '{']) {
        return parsed;
    }
    Ok(Json::String(text.to_string()))
}

/// Splits `"key": rest` into its key and the text after the colon. The key
/// must be a quoted string, so bare text like `https://x` stays a string.
fn split_entry(text: &str) -> Option<(String, &str)> {
    let body = text.trim_start();
    let mut chars = body.char_indices();
    chars.next()?; // opening quote
    let mut end = None;
    let mut escaped = false;
    for (i, c) in chars {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            end = Some(i);
            break;
        }
    }
    let end = end?;
    let key = match Json::parse(&body[..=end]) {
        Ok(Json::String(key)) => key,
        _ => return None,
    };
    let rest = body[end + 1..].trim_start();
    Some((key, rest.strip_prefix(':')?))
}

/// Splits text at commas that sit outside strings and brackets.
fn split_top_commas(text: &str) -> Vec<&str> {
    let mut items = Vec::new();
    let mut start = 0;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, c) in text.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
        } else {
            match c {
                '"' => in_string = true,
                '[' | '{' => depth += 1,
                ']' | '}' => depth = depth.saturating_sub(1),
                ',' if depth == 0 => {
                    items.push(&text[start..i]);
                    start = i + 1;
                }
                _ => {}
            }
        }
    }
    items.push(&text[start..]);
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(src: &str) -> JsonEditorState {
        JsonEditorState::parse(src).unwrap()
    }

    /// Editable field text, for brevity in the tests.
    fn entry(value: &str) -> String {
        value.to_string()
    }

    /// A state whose cursor is on `path`, so tests do not have to count
    /// `select_down` presses through the tree.
    fn at(src: &str, path: &[usize]) -> JsonEditorState {
        let mut state = doc(src);
        state.cursor = path.to_vec();
        state.clamp_field();
        state
    }

    fn assert_valid(state: &JsonEditorState) {
        let mut text = String::new();
        state.root().write_compact(&mut text);
        assert_eq!(&Json::parse(&text).unwrap(), state.root());
    }

    #[test]
    fn edit_returns_the_selected_fields_text() {
        let mut state = doc(r#"{"a": [1], "b": null}"#);
        assert_eq!(state.edit(), "\"a\": [1], \"b\": null", "root value");

        state.select_down();
        assert!(state.select_key());
        assert_eq!(state.edit(), "a");
        assert!(state.select_value());
        assert_eq!(state.edit(), "1", "array items without brackets");

        state.select_down();
        assert_eq!(state.edit(), "1");

        state.select_down();
        assert!(state.select_key());
        assert_eq!(state.edit(), "b");
        assert!(state.select_value());
        assert_eq!(state.edit(), "null", "null is the literal text");
    }

    #[test]
    fn commit_replaces_values_and_converts_types() {
        let mut state = doc(r#"{"a": 1}"#);
        state.select_down();
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
    fn empty_text_is_an_empty_string() {
        let mut state = doc(r#"{"a": 1}"#);
        state.select_down();
        assert!(state.commit(entry("")).is_ok());
        assert_eq!(state.root(), &Json::parse(r#"{"a": ""}"#).unwrap());

        assert!(
            state.commit(entry("null")).is_ok(),
            "null is the literal text"
        );
        assert_eq!(state.root(), &Json::parse(r#"{"a": null}"#).unwrap());
    }

    #[test]
    fn invalid_text_never_reaches_the_document() {
        let mut state = doc(r#"{"a": 1}"#);
        state.select_down();
        let err = state.commit(entry("[1,]")).unwrap_err();
        assert!(matches!(err, EditError::InvalidJson(_)));
        assert!(err.to_string().contains("line 1"));
        assert_eq!(state.root(), &Json::parse(r#"{"a": 1}"#).unwrap());
        assert_valid(&state);
    }

    #[test]
    fn commit_completes_unclosed_values() {
        let mut s = doc(r#"{"a": 1}"#);
        s.select_down();
        assert!(
            s.commit(entry("\"some words ")).is_ok(),
            "missing closing quote"
        );
        assert_eq!(s.root(), &Json::parse(r#"{"a": "some words "}"#).unwrap());

        let mut s = doc(r#"{"a": 1}"#);
        s.select_down();
        assert!(
            s.commit(entry("\"")).is_ok(),
            "a lone quote means empty string"
        );
        assert_eq!(s.root(), &Json::parse(r#"{"a": ""}"#).unwrap());

        let mut s = doc(r#"{"a": 1}"#);
        s.select_down();
        assert!(s.commit(entry("[1, 2")).is_ok(), "missing closing bracket");
        assert_eq!(s.root(), &Json::parse(r#"{"a": [1, 2]}"#).unwrap());

        let mut s = doc(r#"{"a": 1}"#);
        s.select_down();
        assert!(
            s.commit(entry("{\"x\": [9")).is_ok(),
            "missing several closers"
        );
        assert_eq!(s.root(), &Json::parse(r#"{"a": {"x": [9]}}"#).unwrap());

        let mut s = doc(r#"{"a": 1}"#);
        s.select_down();
        assert!(
            s.commit(entry("{")).is_ok(),
            "a lone brace means empty object"
        );
        assert_eq!(s.root(), &Json::parse(r#"{"a": {}}"#).unwrap());
        assert_valid(&s);

        assert!(
            s.commit(entry("[1,]")).is_err(),
            "broken bracketed text is rejected"
        );
        assert!(
            s.commit(entry("{\"a\": }")).is_err(),
            "broken objects are rejected"
        );
    }

    #[test]
    fn commit_detects_types_from_bare_text() {
        for (text, expected) in [
            ("hello world", r#"{"a": "hello world"}"#),
            ("42", r#"{"a": 42}"#),
            ("true", r#"{"a": true}"#),
            ("\"some\"", r#"{"a": "some"}"#),
            ("\"some1\", \"some2\"", r#"{"a": ["some1", "some2"]}"#),
            ("\"x\": true", r#"{"a": {"x": true}}"#),
            ("1, two", r#"{"a": [1, "two"]}"#),
            ("https://example.com", r#"{"a": "https://example.com"}"#),
        ] {
            let mut s = doc(r#"{"a": null}"#);
            s.select_down();
            assert!(s.commit(entry(text)).is_ok(), "{text:?}");
            assert_eq!(s.root(), &Json::parse(expected).unwrap(), "{text:?}");
            assert_valid(&s);
        }
    }

    #[test]
    fn values_round_trip_through_their_editable_text() {
        for src in [
            r#""hello""#,
            r#""hello world ""#,
            r#"42"#,
            r#"1.50"#,
            r#"true"#,
            r#"null"#,
            r#""""#,
            r#"[]"#,
            r#"{}"#,
            r#"["a", "b"]"#,
            r#"[1, [2, 3]]"#,
            r#"{"a": 1, "b": [true, null]}"#,
            r#"{"a": {"b": "x"}}"#,
        ] {
            let value = Json::parse(src).unwrap();
            let text = expose_value(&value);
            let back = interpret_value(&text).unwrap();
            assert_eq!(back, value, "src={src} text={text:?}");
        }
    }

    #[test]
    fn missing_closers_are_detected() {
        assert_eq!(missing_closers("\"some").as_deref(), Some("\""));
        assert_eq!(missing_closers("{\"a\": [1").as_deref(), Some("]}"));
        assert_eq!(missing_closers("[1, 2").as_deref(), Some("]"));
        assert_eq!(missing_closers("[1, 2]").as_deref(), None);
        assert_eq!(missing_closers("\"ok\"").as_deref(), None);
    }

    #[test]
    fn commit_renames_and_checks_keys() {
        let mut state = doc(r#"{"a": 1, "b": 2}"#);
        state.select_down();
        assert!(state.select_key());
        assert!(state.commit("c").is_ok());
        assert_eq!(state.root(), &Json::parse(r#"{"c": 1, "b": 2}"#).unwrap());

        assert_eq!(state.commit("b"), Err(EditError::DuplicateKey("b".into())));
        assert_eq!(state.commit("x\ny"), Err(EditError::MultiLineKey));
        assert_eq!(state.root(), &Json::parse(r#"{"c": 1, "b": 2}"#).unwrap());
    }

    #[test]
    fn committing_one_field_never_touches_the_other() {
        let mut state = doc(r#"{"a": 1}"#);
        state.select_down();
        assert!(state.commit("2").is_ok(), "value commit keeps the key");
        assert_eq!(state.root(), &Json::parse(r#"{"a": 2}"#).unwrap());

        assert!(state.select_key());
        assert!(state.commit("z").is_ok(), "key commit keeps the value");
        assert_eq!(state.root(), &Json::parse(r#"{"z": 2}"#).unwrap());
        assert_valid(&state);
    }

    #[test]
    fn add_delete_and_reorder() {
        let mut state = doc(r#"{"a": []}"#);
        state.select_down();
        state.add_entry().unwrap();
        assert_eq!(state.cursor_path(), [0, 0]);
        assert!(state.commit(entry("")).is_ok());
        assert_eq!(state.root(), &Json::parse(r#"{"a": [""]}"#).unwrap());

        let mut state = doc("[1, 2, 3]");
        state.select_down();
        state.select_down();
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
    fn moving_across_swaps_among_siblings() {
        // A free sibling slot makes it a plain reorder first.
        let mut state = at(r#"{"a": 1, "b": 2, "c": 3}"#, &[0]);
        state.move_entry_across_down().unwrap();
        assert_eq!(
            state.root(),
            &Json::parse(r#"{"b": 2, "a": 1, "c": 3}"#).unwrap(),
            "a takes b's place, keeping its own name and value"
        );
        assert_eq!(state.cursor_path(), [1]);

        // With none, the next property takes the entry in.
        let mut state = at(r#"{"a": 1, "b": {"k": 0}, "c": 3}"#, &[0]);
        state.move_entry_across_down().unwrap();
        assert_eq!(
            state.root(),
            &Json::parse(r#"{"b": {"a": 1, "k": 0}, "c": 3}"#).unwrap()
        );
        assert_eq!(state.cursor_path(), [0, 0]);
        assert_valid(&state);

        // Up takes the property above, as its last child.
        let mut state = at(r#"{"a": {"k": 0}, "b": 1, "c": 3}"#, &[1]);
        state.move_entry_across_up().unwrap();
        assert_eq!(
            state.root(),
            &Json::parse(r#"{"a": {"k": 0, "b": 1}, "c": 3}"#).unwrap()
        );
        assert_eq!(state.cursor_path(), [0, 1]);
        assert_valid(&state);
    }

    #[test]
    fn moving_across_can_leave_an_object_entirely() {
        // No neighbour can hold it, so it leaves the object entirely.
        let mut state = at(r#"{"a": 1, "nest": {"x": 1, "y": 2}, "b": 3}"#, &[1, 1]);
        state.move_entry_across_down().unwrap();
        assert_eq!(
            state.root(),
            &Json::parse(r#"{"a": 1, "nest": {"x": 1}, "y": 2, "b": 3}"#).unwrap(),
            "y takes its place after the object it left"
        );
        assert_eq!(state.cursor_path(), [2]);
        assert_valid(&state);

        let mut state = at(r#"{"a": 1, "nest": {"x": 1, "y": 2}, "b": 3}"#, &[1, 0]);
        state.move_entry_across_up().unwrap();
        assert_eq!(
            state.root(),
            &Json::parse(r#"{"a": 1, "x": 1, "nest": {"y": 2}, "b": 3}"#).unwrap(),
            "x lands before the object it left"
        );
        assert_eq!(state.cursor_path(), [1]);
        assert_valid(&state);

        // Out of a nested object, one level further up.
        let mut state = at(
            r#"{"deep": {"inner": {"k": 1}, "j": 2}, "z": 3}"#,
            &[0, 0, 0],
        );
        state.move_entry_across_down().unwrap();
        assert_eq!(
            state.root(),
            &Json::parse(r#"{"deep": {"inner": {}, "k": 1, "j": 2}, "z": 3}"#).unwrap(),
            "out of a nested object, k lands beside inner, as a child of deep"
        );
        assert_valid(&state);
    }

    #[test]
    fn moving_across_moves_array_items_too() {
        // The last item of an array moves into the array next door.
        let mut state = at(r#"{"arr": [1, 2], "other": [9]}"#, &[0, 1]);
        state.move_entry_across_down().unwrap();
        assert_eq!(
            state.root(),
            &Json::parse(r#"{"arr": [1], "other": [2, 9]}"#).unwrap()
        );
        assert_eq!(state.cursor_path(), [1, 0]);
        assert_valid(&state);

        // Into an object, the item is named after the text it holds.
        let mut state = at(r#"{"arr": [1, 2], "obj": {"x": 1}}"#, &[0, 1]);
        state.move_entry_across_down().unwrap();
        assert_eq!(
            state.root(),
            &Json::parse(r#"{"arr": [1], "obj": {"2": 2, "x": 1}}"#).unwrap()
        );
        assert_valid(&state);

        // Out of an array with a free sibling slot, that is a plain move.
        let mut state = at(r#"[1, 2]"#, &[0]);
        state.move_entry_across_down().unwrap();
        assert_eq!(state.root(), &Json::parse("[2, 1]").unwrap());
    }

    #[test]
    fn moving_across_moves_a_whole_object() {
        // The document's own object has nothing outside it, so its edges are
        // the document's.
        let mut state = at(r#"{"a": {"x": 1}, "b": 2}"#, &[0]);
        assert_eq!(
            state.move_entry_across_up(),
            Err(EditError::Refused(
                "already the first entry of the document"
            ))
        );
        // The last entry of the document has no line to leave for either.
        let mut state = at(r#"{"a": 1, "b": 2}"#, &[1]);
        assert_eq!(
            state.move_entry_across_down(),
            Err(EditError::Refused("already the last entry of the document"))
        );
        // A lone entry is the document's only line, so it stays put and says
        // why rather than vanishing.
        let mut state = at(r#"{"a": {"x": 1}}"#, &[0]);
        assert_eq!(
            state.move_entry_across_down(),
            Err(EditError::Refused("already the last entry of the document"))
        );
        assert_eq!(state.root(), &Json::parse(r#"{"a": {"x": 1}}"#).unwrap());

        // A container below takes it in.
        let mut state = at(r#"{"top": {"a": {"x": 1}}, "b": {"k": 0}}"#, &[0, 0]);
        state.move_entry_across_down().unwrap();
        assert_eq!(
            state.root(),
            &Json::parse(r#"{"top": {}, "b": {"a": {"x": 1}, "k": 0}}"#).unwrap()
        );
        assert_eq!(state.cursor_path(), [1, 0]);
        assert_valid(&state);
    }

    #[test]
    fn moving_across_at_the_edges_explains_itself() {
        // A property of the root object cannot leave it: the document has no
        // line outside.
        let mut state = doc(r#"{"a": 1, "b": 2}"#);
        state.select_down();
        assert_eq!(
            state.move_entry_across_up(),
            Err(EditError::Refused(
                "already the first entry of the document"
            ))
        );
        state.select_down();
        assert_eq!(
            state.move_entry_across_down(),
            Err(EditError::Refused("already the last entry of the document"))
        );
        assert_eq!(state.root(), &Json::parse(r#"{"a": 1, "b": 2}"#).unwrap());

        // A property cannot land in an array, which has no keys: it passes
        // over and leaves the object instead.
        let mut state = at(r#"{"obj": {"x": 1}, "arr": [1]}"#, &[0, 0]);
        state.move_entry_across_down().unwrap();
        assert_eq!(
            state.root(),
            &Json::parse(r#"{"obj": {}, "x": 1, "arr": [1]}"#).unwrap(),
            "the array cannot hold a property, so x leaves obj for the slot before it"
        );
        assert_valid(&state);

        // Nothing above a lone object's first entry, so it lifts out of it
        // and lands beside the object.
        let mut state = at(r#"{"a": {"x": 1}, "z": 9}"#, &[0, 0]);
        state.move_entry_across_up().unwrap();
        assert_eq!(
            state.root(),
            &Json::parse(r#"{"x": 1, "a": {}, "z": 9}"#).unwrap(),
            "x leaves a, landing before it"
        );
        assert_valid(&state);

        let mut state = at(r#"{"a": {"x": 1}}"#, &[0, 0]);
        assert_eq!(
            state.move_entry_across_up(),
            Ok(()),
            "x lifts out of a, which is the document's own object"
        );
        // Down from there puts x back where it was among a's own entries.
        assert_eq!(state.move_entry_across_down(), Ok(()), "x joins a again");
        assert_eq!(state.root(), &Json::parse(r#"{"a": {"x": 1}}"#).unwrap());
    }

    #[test]
    fn hiding_a_block_shortens_the_tree() {
        let src = r#"{"a": {"x": 1, "y": 2}, "b": {"k": 0}}"#;
        let mut state = doc(src);
        let before = state.line_count();
        state.select_down();
        assert_eq!(state.cursor_path(), [0], "a");
        state.collapse_block();
        assert!(state.is_collapsed());
        assert_eq!(
            state.line_count(),
            before - 3,
            "a's two children and its close"
        );
        assert_eq!(
            state.root(),
            &Json::parse(src).unwrap(),
            "the document is untouched"
        );

        // The cursor still lands on a, and moving on visits b next.
        assert_eq!(state.cursor_path(), [0]);
        state.select_down();
        assert_eq!(state.cursor_path(), [1], "b, with a's block hidden");

        state.select_up();
        state.expand_block();
        assert!(!state.is_collapsed());
        assert_eq!(state.line_count(), before, "a's block is back");
    }

    #[test]
    fn hiding_nests_and_unwinds_by_level() {
        let mut state = doc(r#"{"a": {"x": {"deep": 1}, "y": 2}}"#);
        state.select_down();
        state.select_down();
        assert_eq!(state.cursor_path(), [0, 0], "x");
        state.collapse_block();
        assert_eq!(
            state.line_count(),
            6,
            "root, a, x and its close, y, a's close"
        );

        // y is still reachable, and expanding x does not expand a.
        state.select_down();
        assert_eq!(state.cursor_path(), [0, 1], "y");
        state.select_up();
        state.expand_block();
        assert_eq!(state.line_count(), 8, "x's block is back");
        assert!(!state.is_collapsed(), "x is open again");

        // Back up to a, then hide the whole of it.
        state.cursor_to_parent();
        assert_eq!(state.cursor_path(), [0], "a");
        state.collapse_block();
        assert_eq!(state.line_count(), 3, "root, a and the root's close");
        state.expand_block();
        assert_eq!(state.line_count(), 8, "a is open again");
        assert!(state.collapsed_paths().is_empty(), "nothing is hidden");
    }

    #[test]
    fn hiding_leaves_scalars_and_empty_blocks_alone() {
        let mut state = doc(r#"{"a": 1, "b": {}}"#);
        state.select_down();
        state.collapse_block();
        assert!(!state.is_collapsed(), "a scalar has no block to hide");

        state.select_down();
        state.collapse_block();
        assert!(!state.is_collapsed(), "an empty block has nothing to hide");
        assert_eq!(state.line_count(), 4, "root, a, b, and the root's close");
    }

    #[test]
    fn hiding_twice_is_harmless() {
        let mut state = doc(r#"{"a": {"x": 1}}"#);
        state.select_down();
        state.collapse_block();
        let once = state.line_count();
        state.collapse_block();
        assert_eq!(state.line_count(), once, "already hidden");
        assert_valid(&state);
    }

    #[test]
    fn unflattening_an_entry_lifts_its_children() {
        // The children take the entry's place among its siblings, in order.
        let mut state = doc(r#"{"a": {"x": 1, "y": 2}, "b": 3}"#);
        state.select_down();
        state.unflatten_entry().unwrap();
        assert_eq!(
            state.root(),
            &Json::parse(r#"{"x": 1, "y": 2, "b": 3}"#).unwrap(),
            "the wrapper is gone and its children are not"
        );
        assert_eq!(state.cursor_path(), [0], "on the first of them");
        assert_valid(&state);

        // Nested: deleting `inner` keeps its own children, so `deep` is left
        // holding them rather than being emptied.
        let mut state = doc(r#"{"deep": {"inner": {"k": 1}, "j": 2}, "z": 3}"#);
        state.select_down();
        state.select_down();
        assert_eq!(state.cursor_path(), [0, 0], "inner");
        state.unflatten_entry().unwrap();
        assert_eq!(
            state.root(),
            &Json::parse(r#"{"deep": {"k": 1, "j": 2}, "z": 3}"#).unwrap()
        );
        assert_valid(&state);

        // Deleting `deep` instead lifts its children a level further.
        let mut state = doc(r#"{"deep": {"inner": {"k": 1}, "j": 2}, "z": 3}"#);
        state.select_down();
        state.unflatten_entry().unwrap();
        assert_eq!(
            state.root(),
            &Json::parse(r#"{"inner": {"k": 1}, "j": 2, "z": 3}"#).unwrap()
        );
        assert_valid(&state);

        // An array's items are moved, and where they land among named
        // siblings they are given a name from their own text.
        let mut state = doc(r#"{"a": [1, 2], "b": 3}"#);
        state.select_down();
        state.unflatten_entry().unwrap();
        assert_eq!(
            state.root(),
            &Json::parse(r#"{"1": 1, "2": 2, "b": 3}"#).unwrap()
        );
        assert_valid(&state);

        // Deleting an array nested in an array needs no name at all.
        let mut state = doc("[[1, 2], [3]]");
        state.select_down();
        state.unflatten_entry().unwrap();
        assert_eq!(state.root(), &Json::parse("[1, 2, [3]]").unwrap());
        assert_valid(&state);
    }

    #[test]
    fn unflattening_something_with_nothing_to_lift_is_refused() {
        // A scalar has no children to lift; the plain delete is the one that
        // fits.
        let mut state = doc(r#"{"a": 1}"#);
        state.select_down();
        assert_eq!(
            state.unflatten_entry(),
            Err(EditError::Refused("only a container has children to lift"))
        );
        assert_eq!(state.root(), &Json::parse(r#"{"a": 1}"#).unwrap());

        // An empty container: nothing to lift out of it.
        let mut state = doc(r#"{"a": {}, "b": 1}"#);
        state.select_down();
        assert_eq!(
            state.unflatten_entry(),
            Err(EditError::Refused(
                "only a container with entries has children to lift"
            ))
        );
        assert_eq!(state.root(), &Json::parse(r#"{"a": {}, "b": 1}"#).unwrap());

        // A child's key already used beside it: the move would collide.
        let mut state = doc(r#"{"a": {"k": 1}, "k": 2}"#);
        state.select_down();
        assert_eq!(
            state.unflatten_entry(),
            Err(EditError::Refused(
                "a child's key is already used beside this entry"
            ))
        );
        assert_eq!(
            state.root(),
            &Json::parse(r#"{"a": {"k": 1}, "k": 2}"#).unwrap()
        );

        // The document's own root has nowhere to lift into.
        let mut state = doc(r#"{"a": 1}"#);
        assert_eq!(
            state.unflatten_entry(),
            Err(EditError::Refused("cannot delete the root value"))
        );
    }

    #[test]
    fn duplicating_an_entry_copies_it_next_to_itself() {
        let mut state = doc(r#"{"a": 1, "b": 2}"#);
        state.select_down();
        state.duplicate_entry().unwrap();
        assert_eq!(
            state.root(),
            &Json::parse(r#"{"a": 1, "a copy": 1, "b": 2}"#).unwrap(),
            "the copy needs a name of its own"
        );
        assert_eq!(state.cursor_path(), [1], "the copy is selected");
        assert_eq!(state.selected_field(), Field::Key, "ready to be renamed");

        // The copy's key can be edited straight away, which is why it is the
        // selected field.
        assert_eq!(state.edit(), "a copy");
        state.commit(entry("renamed")).unwrap();
        assert_eq!(
            state.root(),
            &Json::parse(r#"{"a": 1, "renamed": 1, "b": 2}"#).unwrap(),
            "committing renames the copy in place"
        );
        assert_valid(&state);

        // Duplicating the copy again keeps the names apart.
        state.duplicate_entry().unwrap();
        assert_eq!(
            state.root(),
            &Json::parse(r#"{"a": 1, "renamed": 1, "renamed copy": 1, "b": 2}"#).unwrap(),
            "the copy of \"renamed\" is named after it"
        );
        assert_eq!(state.cursor_path(), [2]);
        assert_valid(&state);

        // A container comes along whole.
        let mut state = doc(r#"{"a": {"x": [1]}}"#);
        state.select_down();
        state.duplicate_entry().unwrap();
        assert_eq!(
            state.root(),
            &Json::parse(r#"{"a": {"x": [1]}, "a copy": {"x": [1]}}"#).unwrap()
        );
        assert_valid(&state);
    }

    #[test]
    fn duplicating_an_array_item_needs_no_name() {
        let mut state = doc("[1, 2]");
        state.select_down();
        state.duplicate_entry().unwrap();
        assert_eq!(state.root(), &Json::parse("[1, 1, 2]").unwrap());
        assert_eq!(state.cursor_path(), [1], "the copy is selected");
        assert_eq!(state.selected_field(), Field::Value, "an item has no key");
        assert_valid(&state);
    }

    #[test]
    fn the_root_cannot_be_duplicated() {
        let mut state = doc(r#"{"a": 1}"#);
        assert_eq!(
            state.duplicate_entry(),
            Err(EditError::Refused("cannot duplicate the root value"))
        );
        assert_eq!(state.root(), &Json::parse(r#"{"a": 1}"#).unwrap());
    }

    #[test]
    fn a_value_can_be_turned_into_text_and_back() {
        let mut state = doc(r#"{"n": 42, "o": {"a": [1]}, "s": "hi"}"#);
        state.select_down();
        state.value_to_string().unwrap();
        assert_eq!(
            state.root(),
            &Json::parse(r#"{"n": "42", "o": {"a": [1]}, "s": "hi"}"#).unwrap()
        );
        assert_valid(&state);

        // And back again.
        state.parse_value_text().unwrap();
        assert_eq!(
            state.root(),
            &Json::parse(r#"{"n": 42, "o": {"a": [1]}, "s": "hi"}"#).unwrap()
        );

        // A container becomes one string holding its JSON text.
        state.select_down();
        assert_eq!(state.cursor_path(), [1], "o");
        state.value_to_string().unwrap();
        assert_eq!(
            state.root(),
            &Json::parse(r#"{"n": 42, "o": "{\"a\":[1]}", "s": "hi"}"#).unwrap()
        );
        state.parse_value_text().unwrap();
        assert_eq!(
            state.root(),
            &Json::parse(r#"{"n": 42, "o": {"a": [1]}, "s": "hi"}"#).unwrap()
        );
        assert_valid(&state);
    }

    #[test]
    fn turning_a_value_into_text_says_why_not() {
        // A string is already its own text.
        let mut state = doc(r#"{"s": "hi"}"#);
        state.select_down();
        assert_eq!(
            state.value_to_string(),
            Err(EditError::Refused("it is already a string"))
        );
        assert_eq!(state.root(), &Json::parse(r#"{"s": "hi"}"#).unwrap());

        // Only a value, never a key.
        let mut state = doc(r#"{"s": "hi"}"#);
        state.select_down();
        assert!(state.select_key(), "a property has a key");
        assert_eq!(
            state.value_to_string(),
            Err(EditError::Refused(
                "only a value can be turned into a string"
            ))
        );

        // Reading text needs a string to read.
        let mut state = doc(r#"{"n": 42}"#);
        state.select_down();
        assert_eq!(
            state.parse_value_text(),
            Err(EditError::Refused("it is not a string to read"))
        );
    }

    #[test]
    fn text_that_describes_nothing_is_refused() {
        let mut state = doc(r#"["{oops"]"#);
        state.select_down();
        assert!(state.parse_value_text().is_err(), "that is not JSON");
        assert_eq!(
            state.root(),
            &Json::parse(r#"["{oops"]"#).unwrap(),
            "and the string is left alone"
        );

        // A plain word is read as a value, the way commit reads text, so
        // quoting it is what keeps it a string.
        let mut state = doc(r#"["true", "42"]"#);
        state.select_down();
        state.parse_value_text().unwrap();
        assert_eq!(state.root(), &Json::parse(r#"[true, "42"]"#).unwrap());
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
        state.select_down();
        assert_eq!(
            state.move_entry_down(),
            Err(EditError::Refused("already the last entry"))
        );
    }

    #[test]
    fn navigation_follows_preorder() {
        let mut state = doc(r#"{"a": {"b": [1]}}"#);
        assert_eq!(state.path_string(), "root");
        state.select_down();
        assert_eq!(state.path_string(), "root[\"a\"]");
        state.select_down();
        assert_eq!(state.path_string(), "root[\"a\"][\"b\"]");
        state.cursor_to_first_child();
        assert_eq!(state.path_string(), "root[\"a\"][\"b\"][0]");
        assert!(!state.cursor_to_first_child(), "scalars have no children");
        state.cursor_to_parent();
        state.cursor_to_parent();
        assert_eq!(state.path_string(), "root[\"a\"]");
        state.select_up();
        assert_eq!(state.path_string(), "root");
        assert!(!state.select_up(), "already at the top");
    }

    #[test]
    fn viewport_metrics_match_the_tree_layout() {
        let mut state = doc(r#"{"a": [1, 2]}"#);
        assert_eq!(state.line_count(), 6);
        assert_eq!(state.cursor_line(), 0);
        state.select_down();
        assert_eq!(state.cursor_line(), 1);
        state.select_down();
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
    fn select_at_picks_the_node_and_field_under_a_pointer() {
        // Rows: 0 `{`, 1 `  "a": [`, 2 `    1`, 3 `  ]`, 4 `  "b": 2`, 5 `}`.
        let mut s = doc(r#"{"a": [1], "b": 2}"#);
        assert!(
            s.select_at(3, 2),
            "closing rows select the block they close"
        );
        assert_eq!(s.cursor_path(), [0]);
        assert_eq!(
            s.selected_field(),
            Field::Value,
            "the bracket is value text"
        );

        assert!(s.select_at(5, 0), "the root's closing row selects the root");
        assert_eq!(s.cursor_path(), Vec::<usize>::new());

        assert!(s.select_at(1, 3), "on the key");
        assert_eq!(s.cursor_path(), [0]);
        assert_eq!(s.selected_field(), Field::Key);

        assert!(s.select_at(1, 7), "on the value");
        assert_eq!(s.cursor_path(), [0]);
        assert_eq!(s.selected_field(), Field::Value);

        assert!(s.select_at(2, 5), "array element");
        assert_eq!(s.cursor_path(), [0, 0]);
        assert_eq!(s.selected_field(), Field::Value);

        assert!(s.select_at(4, 2), "later entry");
        assert_eq!(s.cursor_path(), [1]);
        assert_eq!(s.selected_field(), Field::Key);
    }

    #[test]
    fn horizontal_scrolling() {
        // `{"a": 1}` rows: `{`, `  "a": 1`, `}` — the widest is 8 columns.
        let mut s = doc(r#"{"a": 1}"#);
        assert_eq!(s.content_width(), 8);
        s.set_scroll_x(99);
        assert_eq!(s.scroll_x(), 7, "clamped to the content");

        s.set_scroll_x(0);
        s.select_down();
        s.ensure_cursor_visible_x(4);
        assert_eq!(s.scroll_x(), 4, "the value is kept in view");
        s.ensure_cursor_visible_x(2);
        assert_eq!(s.scroll_x(), 6);
        s.select_key();
        s.ensure_cursor_visible_x(10);
        assert_eq!(s.scroll_x(), 2, "back to the key");
    }

    #[test]
    fn select_key_and_value() {
        let mut s = doc(r#"{"a": 1}"#);
        s.select_down();
        assert_eq!(s.selected_field(), Field::Value);
        assert!(s.select_key());
        assert_eq!(s.selected_field(), Field::Key);
        assert!(s.select_key(), "already selected still succeeds");
        assert!(s.select_value());
        assert_eq!(s.selected_field(), Field::Value);

        let mut s = doc("[1]");
        s.select_down();
        assert!(!s.select_key(), "array elements have no key");
        assert_eq!(s.selected_field(), Field::Value);

        let mut s = doc(r#"{"a": 1}"#);
        assert!(!s.select_key(), "the root has no key");
    }

    #[test]
    fn select_left_and_right_stay_on_the_level() {
        let mut s = doc(r#"{"a": {"x": 1}, "b": 2}"#);
        s.select_down();
        assert!(s.select_key());
        assert_eq!(s.cursor_path(), [0]);

        assert!(s.select_right(), "key -> value");
        assert_eq!(s.cursor_path(), [0]);
        assert_eq!(s.selected_field(), Field::Value);

        assert!(s.select_right(), "value -> key of the next line");
        assert_eq!(s.cursor_path(), [1], "skips the child of a container");
        assert_eq!(s.selected_field(), Field::Key);

        assert!(s.select_right(), "key -> value");
        assert_eq!(s.cursor_path(), [1]);
        assert_eq!(s.selected_field(), Field::Value);
        assert!(!s.select_right(), "last field of the level");

        assert!(s.select_left(), "value -> key");
        assert_eq!(s.cursor_path(), [1]);
        assert_eq!(s.selected_field(), Field::Key);

        assert!(s.select_left(), "key -> value of the line above");
        assert_eq!(s.cursor_path(), [0]);
        assert_eq!(s.selected_field(), Field::Value);

        assert!(s.select_left(), "value -> key");
        assert_eq!(s.selected_field(), Field::Key);
        assert!(!s.select_left(), "first field of the level");
    }

    #[test]
    fn arrays_select_values_across_lines() {
        let mut s = doc("[1, 2]");
        s.select_down();
        assert!(!s.select_key());
        assert!(s.select_right(), "value -> value of the next line");
        assert_eq!(s.cursor_path(), [1]);
        assert_eq!(s.selected_field(), Field::Value);
        assert!(!s.select_right(), "last field of the level");
        assert!(s.select_left());
        assert_eq!(s.cursor_path(), [0]);
    }

    #[test]
    fn moving_between_lines_keeps_the_field() {
        let mut s = doc(r#"{"a": 1, "b": 2}"#);
        s.select_down();
        assert!(s.select_key());
        s.select_down();
        assert_eq!(s.cursor_path(), [1]);
        assert_eq!(
            s.selected_field(),
            Field::Key,
            "field follows the selection"
        );
        s.select_up();
        assert_eq!(s.selected_field(), Field::Key);
        s.select_up();
        assert_eq!(s.cursor_path(), Vec::<usize>::new());
        assert_eq!(
            s.selected_field(),
            Field::Value,
            "clamped where no key exists"
        );
    }

    #[test]
    fn operations_keep_the_document_valid() {
        let mut state = doc(r#"{"a": [1, 2], "b": null}"#);
        state.select_down();
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
