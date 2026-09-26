//! Document printing. The library never pretty-prints; the app owns this.

use ratatui_json_editor::{quote_string, Json};

/// The app's pretty printer: two-space indent, `": "` and `", "`.
pub(crate) fn pretty(value: &Json) -> String {
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
pub(crate) fn compact(value: &Json) -> String {
    let mut out = String::new();
    value.write_compact(&mut out);
    out
}

fn indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}
