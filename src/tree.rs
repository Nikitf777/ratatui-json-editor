//! Flattened rows of the JSON tree.
//!
//! [`flatten`] is the single source of truth for the tree layout: each node
//! contributes one row (plus a closing row for non-empty containers). Both the
//! widget (rendering) and the state (viewport metrics) are built on it, so they
//! can never disagree about line numbering.

use crate::json::Json;

/// Display width of one indentation level in the rendered tree.
pub(crate) const INDENT_WIDTH: usize = 2;

/// One rendered row of the tree.
pub(crate) struct Row {
    /// Index path of the node this row belongs to; `None` for closing rows.
    pub(crate) path: Option<Vec<usize>>,
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
                path: Some(path.clone()),
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
                    path: None,
                    depth,
                    comma,
                    content: RowContent::Close { is_object: false },
                });
            }
        }
        Json::Object(entries) => {
            let empty = entries.is_empty();
            out.push(Row {
                path: Some(path.clone()),
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
                    path: None,
                    depth,
                    comma,
                    content: RowContent::Close { is_object: true },
                });
            }
        }
        scalar => out.push(Row {
            path: Some(path.clone()),
            depth,
            comma,
            content: RowContent::Scalar {
                key,
                value: scalar.clone(),
            },
        }),
    }
}
