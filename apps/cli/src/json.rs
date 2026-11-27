//! Reading Gateway's JSON answers the way the TypeScript CLI's templates do, and printing JSON as `JSON.stringify(value,
//! null, 2)` prints it (whole numbers without `.0`, floats in JavaScript's form).

use std::io;

use serde::Serialize as _;
use serde_json::ser::{CompactFormatter, Formatter, PrettyFormatter, Serializer};
use serde_json::{Map, Value};

use crate::style::js_number;

/// A string field; `None` when it is missing, null or not a string (`value ?? fallback`).
pub fn s<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// A string field that is set and not empty (`value || fallback`).
pub fn s_nonempty<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    s(value, key).filter(|text| !text.is_empty())
}

/// A number field, 0 when missing.
pub fn n(value: &Value, key: &str) -> f64 {
    value.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

/// A number field, `None` when missing or null.
pub fn n_opt(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(Value::as_f64)
}

/// An array field, empty when missing.
pub fn a<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// JavaScript truthiness of a field.
pub fn t(value: &Value, key: &str) -> bool {
    value.get(key).is_some_and(truthy)
}

pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(number) => number.as_f64().is_some_and(|x| x != 0.0 && !x.is_nan()),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// Whether a field is present and not null (`value !== undefined && value !== null`).
pub fn has(value: &Value, key: &str) -> bool {
    value.get(key).is_some_and(|v| !v.is_null())
}

/// `String(value)` as a template literal prints it.
pub fn js_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(number)) => js_number(number.as_f64().unwrap_or(0.0)),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                if item.is_null() {
                    String::new()
                } else {
                    js_string(Some(item))
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".into(),
    }
}

/// Whole floating-point numbers as integers, recursively, as JavaScript prints them.
pub fn normalize(value: &Value) -> Value {
    match value {
        Value::Number(number) if number.is_f64() => {
            let x = number.as_f64().unwrap_or(0.0);
            if x.fract() == 0.0 && x.abs() < 9.007_199_254_740_992e15 {
                Value::from(x as i64)
            } else {
                value.clone()
            }
        }
        Value::Array(items) => Value::Array(items.iter().map(normalize).collect()),
        Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), normalize(v))).collect()),
        _ => value.clone(),
    }
}

/// `JSON.stringify(value, null, 2)`.
pub fn pretty(value: &Value) -> String {
    write(value, PrettyFormatter::with_indent(b"  "))
}

/// `JSON.stringify(value)`.
pub fn compact(value: &Value) -> String {
    write(value, CompactFormatter)
}

fn write(value: &Value, layout: impl Formatter) -> String {
    let mut out = Vec::new();
    let mut serializer = Serializer::with_formatter(&mut out, JsFloats(layout));
    match normalize(value).serialize(&mut serializer) {
        Ok(()) => String::from_utf8(out).unwrap_or_default(),
        Err(_) => String::new(),
    }
}

/// Floats as JavaScript writes them (`0.000007`, where serde_json's ryu writes `7e-6`), in the layout of `F`.
struct JsFloats<F>(F);

impl<F: Formatter> Formatter for JsFloats<F> {
    fn write_f64<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        writer.write_all(js_number(value).as_bytes())
    }

    fn begin_array<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.begin_array(writer)
    }

    fn end_array<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.end_array(writer)
    }

    fn begin_array_value<W: ?Sized + io::Write>(&mut self, writer: &mut W, first: bool) -> io::Result<()> {
        self.0.begin_array_value(writer, first)
    }

    fn end_array_value<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.end_array_value(writer)
    }

    fn begin_object<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.begin_object(writer)
    }

    fn end_object<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.end_object(writer)
    }

    fn begin_object_key<W: ?Sized + io::Write>(&mut self, writer: &mut W, first: bool) -> io::Result<()> {
        self.0.begin_object_key(writer, first)
    }

    fn end_object_key<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.end_object_key(writer)
    }

    fn begin_object_value<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.begin_object_value(writer)
    }

    fn end_object_value<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.end_object_value(writer)
    }
}

/// An object from `(key, value)` pairs, leaving out the `None` ones (JavaScript drops `undefined` fields).
pub fn obj(entries: Vec<(&str, Option<Value>)>) -> Value {
    let mut map = Map::new();
    for (key, value) in entries {
        if let Some(value) = value {
            map.insert(key.to_string(), value);
        }
    }
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn prints_like_json_stringify() {
        assert_eq!(
            pretty(&json!({ "a": 5.0, "b": [0.5, 2.0], "c": {} })),
            "{\n  \"a\": 5,\n  \"b\": [\n    0.5,\n    2\n  ],\n  \"c\": {}\n}"
        );
        assert_eq!(js_string(Some(&json!(["a", 1]))), "a,1");
        assert_eq!(js_string(Some(&json!(3.0))), "3");
    }

    #[test]
    fn prints_floats_as_json_stringify_does() {
        let floats = json!([
            0.000007,
            7e-7,
            123.456,
            1e20,
            1e21,
            1.5e300,
            -0.0000123,
            9007199254740994.0
        ]);
        assert_eq!(
            compact(&floats),
            "[0.000007,7e-7,123.456,100000000000000000000,1e+21,1.5e+300,-0.0000123,9007199254740994]"
        );
        assert_eq!(pretty(&json!({ "cost": 0.000007 })), "{\n  \"cost\": 0.000007\n}");
    }
}
