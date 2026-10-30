//! A small JSON value and writer for error reports and `capabilities` (no dependency).

use std::fmt::Write;

/// A JSON value. Objects keep their keys in insertion order (stable output).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Str(String),
    List(Vec<Value>),
    Obj(Vec<(String, Value)>),
}

impl Value {
    /// An object from `(key, value)` pairs, in order.
    pub fn obj<K: Into<String>>(pairs: impl IntoIterator<Item = (K, Value)>) -> Value {
        Value::Obj(pairs.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    /// A list of values.
    pub fn list<T: Into<Value>>(items: impl IntoIterator<Item = T>) -> Value {
        Value::List(items.into_iter().map(Into::into).collect())
    }

    /// The value at `key` of an object.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::UInt(n) => Some(*n),
            Value::Int(n) => u64::try_from(*n).ok(),
            _ => None,
        }
    }

    /// Compact JSON, one line.
    pub fn to_json(&self) -> String {
        let mut s = String::new();
        self.write(&mut s, None, 0);
        s
    }

    /// Indented JSON (two spaces), ending without a newline.
    pub fn to_json_pretty(&self) -> String {
        let mut s = String::new();
        self.write(&mut s, Some(2), 0);
        s
    }

    fn write(&self, out: &mut String, indent: Option<usize>, depth: usize) {
        let nl = |out: &mut String, d: usize| {
            if let Some(n) = indent {
                out.push('\n');
                out.extend(std::iter::repeat(' ').take(n * d));
            }
        };
        match self {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Int(n) => {
                let _ = write!(out, "{n}");
            }
            Value::UInt(n) => {
                let _ = write!(out, "{n}");
            }
            Value::Str(s) => string(out, s),
            Value::List(items) => {
                if items.is_empty() {
                    out.push_str("[]");
                    return;
                }
                // short lists of scalars stay on one line in pretty output
                let flat = items
                    .iter()
                    .all(|v| !matches!(v, Value::List(_) | Value::Obj(_)));
                out.push('[');
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                        if flat && indent.is_some() {
                            out.push(' ');
                        }
                    }
                    if !flat {
                        nl(out, depth + 1);
                    }
                    v.write(out, indent, depth + 1);
                }
                if !flat {
                    nl(out, depth);
                }
                out.push(']');
            }
            Value::Obj(m) => {
                if m.is_empty() {
                    out.push_str("{}");
                    return;
                }
                out.push('{');
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    nl(out, depth + 1);
                    string(out, k);
                    out.push(':');
                    if indent.is_some() {
                        out.push(' ');
                    }
                    v.write(out, indent, depth + 1);
                }
                nl(out, depth);
                out.push('}');
            }
        }
    }
}

fn string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{2028}' || c == '\u{2029}' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Str(s.to_owned())
    }
}
impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Str(s)
    }
}
impl From<&String> for Value {
    fn from(s: &String) -> Self {
        Value::Str(s.clone())
    }
}
impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}
impl From<u8> for Value {
    fn from(n: u8) -> Self {
        Value::UInt(n.into())
    }
}
impl From<u32> for Value {
    fn from(n: u32) -> Self {
        Value::UInt(n.into())
    }
}
impl From<u64> for Value {
    fn from(n: u64) -> Self {
        Value::UInt(n)
    }
}
impl From<usize> for Value {
    fn from(n: usize) -> Self {
        Value::UInt(n as u64)
    }
}
impl From<i64> for Value {
    fn from(n: i64) -> Self {
        Value::Int(n)
    }
}
impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(o: Option<T>) -> Self {
        o.map_or(Value::Null, Into::into)
    }
}
impl From<(usize, usize)> for Value {
    /// A line range `[first, last]`.
    fn from((a, b): (usize, usize)) -> Self {
        Value::List(vec![Value::from(a), Value::from(b)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_and_orders() {
        let v = Value::obj([
            ("b", Value::from("a\"b\\c\n\u{1}")),
            ("a", Value::list([1u64, 2])),
            ("n", Value::Null),
        ]);
        assert_eq!(v.to_json(), r#"{"b":"a\"b\\c\n\u0001","a":[1,2],"n":null}"#);
        assert_eq!(
            v.to_json_pretty(),
            "{\n  \"b\": \"a\\\"b\\\\c\\n\\u0001\",\n  \"a\": [1, 2],\n  \"n\": null\n}"
        );
    }
}
