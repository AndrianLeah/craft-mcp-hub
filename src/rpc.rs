//! Small helpers for newline-delimited JSON-RPC.

use serde_json::{json, Map, Value};
use std::io::BufRead;

/// The next line without its line ending, or `None` at end of stream (or on a read error or timeout).
/// Invalid UTF-8 is replaced rather than treated as the end.
pub fn read_line<R: BufRead>(reader: &mut R) -> Option<String> {
    let mut buf = Vec::new();
    match reader.read_until(b'\n', &mut buf) {
        Ok(0) | Err(_) => None,
        Ok(_) => {
            while matches!(buf.last(), Some(b'\n' | b'\r')) {
                buf.pop();
            }
            Some(String::from_utf8_lossy(&buf).into_owned())
        }
    }
}

pub fn result(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

pub fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// `name version` from an `initialize` result's `serverInfo`.
pub fn server_label(init: &Map<String, Value>) -> String {
    let info = init.get("serverInfo");
    let field = |key: &str| info.and_then(|i| i.get(key)).and_then(Value::as_str).unwrap_or("");
    format!("{} {}", field("name"), field("version")).trim().to_string()
}

/// `text` cut to at most `max` characters, with an ellipsis when cut.
pub fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let mut cut: String = text.chars().take(max).collect();
        cut.push('…');
        cut
    }
}
