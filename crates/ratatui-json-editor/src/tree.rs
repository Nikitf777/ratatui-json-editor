//! Flattened rows of the JSON tree.
//!
//! [`flatten`] is the single source of truth for the tree layout: each node
//! contributes one row (plus a closing row for non-empty containers). Both the
//! widget (rendering) and the state (viewport metrics) are built on it, so they
//! can never disagree about line numbering.

use crate::json::{quote_string, Json};
use unicode_width::UnicodeWidthStr;

/// Display width of one indentation level in the rendered tree.
pub(crate) const INDENT_WIDTH: usize = 2;

/// One rendered row of the tree.
pub(crate) struct Row {
    /// Index path of the node this row belongs to. A closing row belongs to
    /// the container it closes.
    pub(crate) path: Vec<usize>,
    /// Nesting depth (the row's indent is `depth * 2` spaces).
    pub(crate) depth: usize,
    /// Whether a separating comma follows this row's content.
    pub(crate) comma: bool,
    /// What the row shows.
    pub(crate) content: RowContent,
}

/// What a [`Row`] shows.
pub(crate) enum RowContent {
    /// A container's opening row (`"key": [` / `{`), or `[]` / `{}` when empty.
    Container {
        key: Option<String>,
        is_object: bool,
        empty: bool,
    },
    /// A scalar node (`"key": 42`).
    Scalar { key: Option<String>, value: Json },
    /// A container's closing row (`]` / `}`).
    Close { is_object: bool },
}

/// The scalar's rendered text, e.g. `"quoted"` for strings.
pub(crate) fn scalar_text(value: &Json) -> String {
    match value {
        Json::String(s) => quote_string(s),
        Json::Number(n) => n.to_string(),
        Json::Bool(b) => b.to_string(),
        _ => "null".to_string(),
    }
}

/// Display column ranges of a row's parts: the key (including its `: `
/// separator) and the value. Both start after the indent.
pub(crate) fn field_spans(row: &Row) -> (Option<(usize, usize)>, (usize, usize)) {
    let indent = INDENT_WIDTH * row.depth;
    let mut value_start = indent;
    let key = row_key(row).map(|key| {
        let end = indent + quote_string(key).width() + 2;
        value_start = end;
        (indent, end)
    });
    let value_width = match &row.content {
        RowContent::Container { empty, .. } => usize::from(*empty) + 1,
        RowContent::Scalar { value, .. } => scalar_text(value).width(),
        RowContent::Close { .. } => 1,
    };
    (key, (value_start, value_start + value_width))
}

/// Display width of a whole row, including its trailing comma.
pub(crate) fn row_width(row: &Row) -> usize {
    let (key, value) = field_spans(row);
    let end = value.1.max(key.map_or(0, |(_, end)| end));
    end + usize::from(row.comma)
}

fn row_key(row: &Row) -> Option<&str> {
    match &row.content {
        RowContent::Container { key, .. } | RowContent::Scalar { key, .. } => key.as_deref(),
        RowContent::Close { .. } => None,
    }
}

/// Flattens the tree into rows in pre-order.
pub(crate) fn flatten(root: &Json) -> Vec<Row> {
    let mut rows = Vec::new();
    push(root, None, &mut Vec::new(), 0, false, &mut rows);
    rows
}

fn push(
    node: &Json,
    key: Option<&str>,
    path: &mut Vec<usize>,
    depth: usize,
    comma: bool,
    out: &mut Vec<Row>,
) {
    let key = key.map(str::to_string);
    match node {
        Json::Array(items) => {
            let empty = items.is_empty();
            out.push(Row {
                path: path.clone(),
                depth,
                comma,
                content: RowContent::Container {
                    key,
                    is_object: false,
                    empty,
                },
            });
            for (index, item) in items.iter().enumerate() {
                path.push(index);
                push(item, None, path, depth + 1, index + 1 < items.len(), out);
                path.pop();
            }
            if !empty {
                out.push(Row {
                    path: path.clone(),
                    depth,
                    comma,
                    content: RowContent::Close { is_object: false },
                });
            }
        }
        Json::Object(entries) => {
            let empty = entries.is_empty();
            out.push(Row {
                path: path.clone(),
                depth,
                comma,
                content: RowContent::Container {
                    key,
                    is_object: true,
                    empty,
                },
            });
            for (index, (entry_key, value)) in entries.iter().enumerate() {
                path.push(index);
                push(
                    value,
                    Some(entry_key),
                    path,
                    depth + 1,
                    index + 1 < entries.len(),
                    out,
                );
                path.pop();
            }
            if !empty {
                out.push(Row {
                    path: path.clone(),
                    depth,
                    comma,
                    content: RowContent::Close { is_object: true },
                });
            }
        }
        scalar => out.push(Row {
            path: path.clone(),
            depth,
            comma,
            content: RowContent::Scalar {
                key,
                value: scalar.clone(),
            },
        }),
    }
}
