//! JavaScript-semantics helpers: string coercion, document-ID identity, and
//! JSON writing that matches `JSON.stringify` output.

use std::borrow::Cow;

use serde::Serialize;
use serde_json::{Map as JsonMap, Number, Value};

pub fn as_f64(value: &Value) -> Option<f64> {
    value.as_f64()
}

/// Like `js_to_string`, but borrows when the value is already a string —
/// the common case for document fields.
pub fn js_text(value: &Value) -> Cow<'_, str> {
    match value {
        Value::String(s) => Cow::Borrowed(s),
        other => Cow::Owned(js_to_string(other)),
    }
}

/// Renders a JSON value the way JavaScript's string coercion would, for use
/// in error messages and default field stringification.
pub fn js_to_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => js_number_string(n),
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(js_to_string).collect::<Vec<_>>().join(","),
        Value::Object(_) => "[object Object]".to_string(),
    }
}

fn js_number_string(number: &Number) -> String {
    if let Some(f) = number.as_f64() {
        if f.fract() == 0.0 && f.abs() < 9_007_199_254_740_992.0 {
            return format!("{}", f as i64);
        }
    }
    number.to_string()
}

/// A document-ID identity key. JavaScript `Map` keys follow SameValueZero:
/// the number `1` and the string `"1"` are distinct, and numbers compare by
/// value (JavaScript numbers are f64, so numbers are keyed by their bits,
/// with -0 normalized to 0). An enum avoids string formatting on every ID
/// operation.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum IdKey {
    Num(u64),
    Str(Box<str>),
    Bool(bool),
    Null,
    Other(Box<str>),
}

pub fn id_key(id: &Value) -> IdKey {
    match id {
        Value::Number(n) => {
            let float = n.as_f64().unwrap_or(f64::NAN);
            let float = if float == 0.0 { 0.0 } else { float };
            IdKey::Num(float.to_bits())
        }
        Value::String(s) => IdKey::Str(s.as_str().into()),
        Value::Bool(b) => IdKey::Bool(*b),
        Value::Null => IdKey::Null,
        other => IdKey::Other(other.to_string().into_boxed_str()),
    }
}

/// Merges two plain objects shallowly, like a JavaScript object spread.
pub fn shallow_merge(base: &Value, overlay: &Value) -> Value {
    let mut merged = base.as_object().cloned().unwrap_or_default();
    if let Some(object) = overlay.as_object() {
        for (key, value) in object {
            merged.insert(key.clone(), value.clone());
        }
    }
    Value::Object(merged)
}

pub fn without_key(value: &Value, key: &str) -> Value {
    let mut object: JsonMap<String, Value> = value.as_object().cloned().unwrap_or_default();
    object.remove(key);
    Value::Object(object)
}

// -- JSON writing ------------------------------------------------------------

pub fn json_string_to(out: &mut String, text: &str) {
    // Fast path: nothing to escape, the overwhelmingly common case for terms
    // and field names. Multi-byte UTF-8 units are >= 0x80 and never escape.
    if text.bytes().all(|b| b != b'"' && b != b'\\' && b >= 0x20) {
        out.push('"');
        out.push_str(text);
        out.push('"');
        return;
    }
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", ch as u32));
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
}

/// Writes a number as JSON. Non-finite values become `null`, exactly like
/// `JSON.stringify(NaN)`.
pub fn json_number_to(out: &mut String, value: f64) {
    match Number::from_f64(value) {
        Some(number) => out.push_str(&number.to_string()),
        None => out.push_str("null"),
    }
}

pub fn json_value_to(out: &mut String, value: &impl Serialize) {
    match serde_json::to_string(value) {
        Ok(text) => out.push_str(&text),
        Err(_) => out.push_str("null"),
    }
}
