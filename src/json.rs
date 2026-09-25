//! A JSON document model that is valid by construction.
//!
//! [`Json`] can only represent documents that serialize to valid JSON: numbers
//! are validated when created ([`Number::new`]), strings and keys are escaped on
//! serialization, and the parser rejects anything that is not well-formed JSON.
//! This is the foundation of the "always valid JSON" guarantee of the editor:
//! every state of the model serializes to valid JSON.

use std::fmt;

/// Maximum nesting depth accepted by [`Json::parse`].
const MAX_DEPTH: usize = 128;

/// A JSON value.
///
/// Object entries keep their insertion order and their keys are unique, which
/// keeps the serialized output stable and easy to edit.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    /// `null`
    Null,
    /// `true` / `false`
    Bool(bool),
    /// A number such as `42` or `-1.5e3`
    Number(Number),
    /// A string value
    String(String),
    /// An array of values
    Array(Vec<Json>),
    /// An object: ordered, unique key/value pairs
    Object(Vec<(String, Json)>),
}

/// A JSON number kept as the exact text it was written as.
///
/// The inner text is guaranteed to be a valid JSON number per RFC 8259
/// (`-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?`), so `Number` always
/// serializes to valid JSON and never rewrites `1.50` into `1.5`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Number(String);

impl Number {
    /// Validates `text` as a JSON number and wraps it.
    pub fn new(text: &str) -> Option<Self> {
        is_number(text).then(|| Self(text.to_string()))
    }

    /// The number exactly as it was written.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<i64> for Number {
    fn from(v: i64) -> Self {
        Self(v.to_string())
    }
}

impl From<u64> for Number {
    fn from(v: u64) -> Self {
        Self(v.to_string())
    }
}

impl fmt::Display for Number {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A syntax error found while parsing JSON.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    /// What is wrong, in words.
    pub message: String,
    /// 1-based line of the offending input.
    pub line: usize,
    /// 1-based column of the offending input.
    pub column: usize,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid JSON at line {}, column {}: {}",
            self.line, self.column, self.message
        )
    }
}

impl std::error::Error for ParseError {}

impl Json {
    /// Parses `src` as a complete JSON document.
    pub fn parse(src: &str) -> Result<Self, ParseError> {
        let mut parser = Parser { src, pos: 0 };
        let value = parser.value(0)?;
        parser.skip_ws();
        if parser.pos < parser.src.len() {
            return Err(parser.error("unexpected trailing input"));
        }
        Ok(value)
    }

    /// The name of this value's type: `null`, `bool`, `number`, `string`,
    /// `array` or `object`.
    pub fn type_name(&self) -> &'static str {
        match self {
            Json::Null => "null",
            Json::Bool(_) => "bool",
            Json::Number(_) => "number",
            Json::String(_) => "string",
            Json::Array(_) => "array",
            Json::Object(_) => "object",
        }
    }

    /// Writes the value as compact JSON text (no insignificant whitespace).
    ///
    /// A small building block — the editing protocol uses it for nested items
    /// in [`crate::JsonEditorState::edit`]. There is deliberately no pretty
    /// printer: how documents are formatted for output is the consumer's
    /// decision (`Json` is a plain enum to walk and print however you like).
    pub fn write_compact(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Number(n) => out.push_str(n.as_str()),
            Json::String(s) => out.push_str(&quote_string(s)),
            Json::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write_compact(out);
                }
                out.push(']');
            }
            Json::Object(entries) => {
                out.push('{');
                for (i, (key, value)) in entries.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(&quote_string(key));
                    out.push(':');
                    value.write_compact(out);
                }
                out.push('}');
            }
        }
    }
}

/// Quotes and escapes `s` as a JSON string literal (including the quotes).
pub fn quote_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn is_number(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    if b.get(i) == Some(&b'-') {
        i += 1;
    }
    match b.get(i) {
        Some(b'0') => i += 1,
        Some(c) if c.is_ascii_digit() => {
            while b.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
        }
        _ => return false,
    }
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return false;
        }
    }
    if matches!(b.get(i), Some(b'e') | Some(b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+') | Some(b'-')) {
            i += 1;
        }
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return false;
        }
    }
    i == b.len()
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn error(&self, message: impl Into<String>) -> ParseError {
        let consumed = &self.src[..self.pos.min(self.src.len())];
        let line = consumed.matches('\n').count() + 1;
        let line_start = consumed.rfind('\n').map_or(0, |i| i + 1);
        let column = consumed[line_start..].chars().count() + 1;
        ParseError {
            message: message.into(),
            line,
            column,
        }
    }

    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, expected: char) -> Result<(), ParseError> {
        match self.peek() {
            Some(c) if c == expected => {
                self.bump();
                Ok(())
            }
            Some(c) => Err(self.error(format!("expected '{expected}' but found '{c}'"))),
            None => Err(self.error(format!("expected '{expected}' but found end of input"))),
        }
    }

    fn literal(&mut self, text: &str, value: Json) -> Result<Json, ParseError> {
        if self.src[self.pos..].starts_with(text) {
            self.pos += text.len();
            Ok(value)
        } else {
            Err(self.error(format!("expected '{text}'")))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, ParseError> {
        if depth > MAX_DEPTH {
            return Err(self.error("nesting is too deep"));
        }
        self.skip_ws();
        match self.peek() {
            Some('{') => self.object(depth),
            Some('[') => self.array(depth),
            Some('"') => Ok(Json::String(self.string()?)),
            Some('t') => self.literal("true", Json::Bool(true)),
            Some('f') => self.literal("false", Json::Bool(false)),
            Some('n') => self.literal("null", Json::Null),
            Some(c) if c == '-' || c.is_ascii_digit() => self.number(),
            Some(c) => Err(self.error(format!("unexpected character '{c}'"))),
            None => Err(self.error("unexpected end of input")),
        }
    }

    fn number(&mut self) -> Result<Json, ParseError> {
        let start = self.pos;
        while matches!(self.peek(), Some(c) if matches!(c, '-' | '+' | '.' | 'e' | 'E') || c.is_ascii_digit())
        {
            self.bump();
        }
        Number::new(&self.src[start..self.pos])
            .map(Json::Number)
            .ok_or_else(|| self.error("invalid number"))
    }

    fn string(&mut self) -> Result<String, ParseError> {
        self.expect('"')?;
        let mut out = String::new();
        loop {
            match self.bump() {
                Some('"') => return Ok(out),
                Some('\\') => match self.bump() {
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some('/') => out.push('/'),
                    Some('b') => out.push('\u{8}'),
                    Some('f') => out.push('\u{c}'),
                    Some('n') => out.push('\n'),
                    Some('r') => out.push('\r'),
                    Some('t') => out.push('\t'),
                    Some('u') => out.push(self.unicode_escape()?),
                    Some(c) => return Err(self.error(format!("invalid escape '\\{c}'"))),
                    None => return Err(self.error("unterminated escape sequence")),
                },
                Some(c) if (c as u32) < 0x20 => {
                    return Err(self.error("unescaped control character in string"));
                }
                Some(c) => out.push(c),
                None => return Err(self.error("unterminated string")),
            }
        }
    }

    fn unicode_escape(&mut self) -> Result<char, ParseError> {
        let first = self.hex4()?;
        if (0xDC00..=0xDFFF).contains(&first) {
            return Err(self.error("unpaired low surrogate in \\u escape"));
        }
        if !(0xD800..=0xDBFF).contains(&first) {
            return char::from_u32(first as u32)
                .ok_or_else(|| self.error("invalid \\u escape"));
        }
        if self.peek() != Some('\\') {
            return Err(self.error("unpaired high surrogate in \\u escape"));
        }
        self.bump();
        if self.peek() != Some('u') {
            return Err(self.error("unpaired high surrogate in \\u escape"));
        }
        self.bump();
        let second = self.hex4()?;
        if !(0xDC00..=0xDFFF).contains(&second) {
            return Err(self.error("invalid surrogate pair in \\u escapes"));
        }
        let code = 0x10000 + (((first as u32) - 0xD800) << 10) + ((second as u32) - 0xDC00);
        char::from_u32(code).ok_or_else(|| self.error("invalid surrogate pair in \\u escapes"))
    }

    fn hex4(&mut self) -> Result<u16, ParseError> {
        let mut value = 0u16;
        for _ in 0..4 {
            let c = self
                .bump()
                .ok_or_else(|| self.error("incomplete \\u escape"))?;
            let digit = c
                .to_digit(16)
                .ok_or_else(|| self.error("invalid hex digit in \\u escape"))? as u16;
            value = value * 16 + digit;
        }
        Ok(value)
    }

    fn array(&mut self, depth: usize) -> Result<Json, ParseError> {
        self.expect('[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(']') {
            self.bump();
            return Ok(Json::Array(items));
        }
        loop {
            items.push(self.value(depth + 1)?);
            self.skip_ws();
            match self.bump() {
                Some(',') => {
                    self.skip_ws();
                    if self.peek() == Some(']') {
                        return Err(self.error("trailing comma in array"));
                    }
                }
                Some(']') => return Ok(Json::Array(items)),
                Some(c) => return Err(self.error(format!("expected ',' or ']' but found '{c}'"))),
                None => return Err(self.error("unterminated array")),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<Json, ParseError> {
        self.expect('{')?;
        let mut entries: Vec<(String, Json)> = Vec::new();
        self.skip_ws();
        if self.peek() == Some('}') {
            self.bump();
            return Ok(Json::Object(entries));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some('"') {
                return Err(self.error("expected a string key"));
            }
            let key = self.string()?;
            if entries.iter().any(|(k, _)| *k == key) {
                return Err(self.error(format!("duplicate key {}", quote_string(&key))));
            }
            self.skip_ws();
            self.expect(':')?;
            let value = self.value(depth + 1)?;
            entries.push((key, value));
            self.skip_ws();
            match self.bump() {
                Some(',') => {
                    self.skip_ws();
                    if self.peek() == Some('}') {
                        return Err(self.error("trailing comma in object"));
                    }
                }
                Some('}') => return Ok(Json::Object(entries)),
                Some(c) => return Err(self.error(format!("expected ',' or '}}' but found '{c}'"))),
                None => return Err(self.error("unterminated object")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(s: &str) -> Json {
        Json::Number(Number::new(s).unwrap())
    }

    #[test]
    fn parses_all_types() {
        let doc = r#"{"a": [1, -2.5e+3, true, false, null], "b": "x\ty", "c": {}}"#;
        let value = Json::parse(doc).unwrap();
        assert_eq!(
            value,
            Json::Object(vec![
                (
                    "a".into(),
                    Json::Array(vec![
                        num("1"),
                        num("-2.5e+3"),
                        Json::Bool(true),
                        Json::Bool(false),
                        Json::Null,
                    ])
                ),
                ("b".into(), Json::String("x\ty".into())),
                ("c".into(), Json::Object(vec![])),
            ])
        );
    }

    #[test]
    fn preserves_number_text() {
        let value = Json::parse("[1.50, 1e3, -0, 0]").unwrap();
        let mut text = String::new();
        value.write_compact(&mut text);
        assert_eq!(text, "[1.50,1e3,-0,0]");
    }

    #[test]
    fn number_grammar_is_strict() {
        for valid in ["0", "-0", "1", "42", "-3", "1.5", "0.0", "1e3", "1E+3", "1.5e-3"] {
            assert!(Number::new(valid).is_some(), "{valid} should be a number");
        }
        for invalid in ["", "-", "+1", "01", "00", "1.", ".5", "1e", "1e+", "0x1", "1.5.6", "1,5"] {
            assert!(Number::new(invalid).is_none(), "{invalid} should not be a number");
        }
    }

    #[test]
    fn roundtrips_through_compact_text() {
        let doc = r#"{"name":"café \u20ac","list":[[],{},null,["deep"]],"n":1.50,"ok":true}"#;
        let value = Json::parse(doc).unwrap();
        let mut text = String::new();
        value.write_compact(&mut text);
        assert_eq!(Json::parse(&text).unwrap(), value);
    }

    #[test]
    fn escapes_and_decodes_strings() {
        let value = Json::parse(r#"["\"\\\/\b\f\n\r\t\u0041\u00e9\ud83d\ude00"]"#).unwrap();
        assert_eq!(
            value,
            Json::Array(vec![Json::String("\"\\/\u{8}\u{c}\n\r\tAé\u{1f600}".into())])
        );
        let text = "quote\" backslash\\ nl\n ctrl\u{1} uni é";
        assert_eq!(
            Json::parse(&quote_string(text)).unwrap(),
            Json::String(text.into())
        );
    }

    #[test]
    fn rejects_malformed_input() {
        for bad in [
            "",
            "   ",
            "{",
            "[1,]",
            "{'a': 1}",
            "{1: 2}",
            "{\"a\": 1, \"a\": 2}",
            "{\"a\" 1}",
            "[1 2]",
            "1 2",
            "\"unterminated",
            "\"bad \\escape\"",
            "\"\\u12\"",
            "\"\\ud800\"",
            "\"raw \n newline\"",
            "nul",
            "01",
        ] {
            assert!(Json::parse(bad).is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn errors_have_positions() {
        let err = Json::parse("{\n  \"a\": oops\n}").unwrap_err();
        assert_eq!(err.line, 2);
        assert!(err.column >= 8, "column was {}", err.column);
        assert!(err.to_string().contains("line 2"));
    }

    #[test]
    fn rejects_deep_nesting() {
        let deep = format!("{}{}", "[".repeat(MAX_DEPTH + 2), "]".repeat(MAX_DEPTH + 2));
        assert!(Json::parse(&deep).is_err());
    }
}
