//! JavaScript-semantics helpers: string coercion, document-ID identity, and
//! JSON writing that matches `JSON.stringify` output.

use serde_json::{Map as JsonMap, Number, Value};

pub fn as_f64(value: &Value) -> Option<f64> {
    value.as_f64()
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

/// A stable identity key for a document ID value. JavaScript distinguishes
/// the number `1` from the string `"1"`, so keys are namespaced by type.
pub fn id_key(id: &Value) -> String {
    match id {
        Value::String(s) => format!("s:{s}"),
        Value::Number(n) => format!("n:{}", js_number_string(n)),
        Value::Bool(b) => format!("b:{b}"),
        Value::Null => "null".to_string(),
        other => format!("j:{other}"),
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

pub fn json_value_to(out: &mut String, value: &Value) {
    match serde_json::to_string(value) {
        Ok(text) => out.push_str(&text),
        Err(_) => out.push_str("null"),
    }
}
