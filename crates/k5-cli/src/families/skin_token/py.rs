//! The bits of Python the skin-token, api, api-key, skin and publish
//! readers share, each with the exception Python 3.12 raises at a line.
//! Kept in this lane's own module until the shared helpers have a home.

use super::super::{get, py_space, Raise};
use crate::out::json::{self, Text, Value};
use crate::out::py;

/// `d.get(key)` on a value already known to be a dict (`None` otherwise).
pub fn at<'a>(d: &'a Value, key: &str) -> Option<&'a Value> {
    match d {
        Value::Object(pairs) => get(pairs, key),
        _ => None,
    }
}

/// `d.get(key, default)` on a dict.
pub fn at_or(d: &Value, key: &str, default: Value) -> Value {
    at(d, key).cloned().unwrap_or(default)
}

/// `x or y`.
pub fn or(x: Option<&Value>, y: Value) -> Value {
    x.filter(|v| py::truthy(v)).cloned().unwrap_or(y)
}

/// A string value.
pub fn s(text: &str) -> Value {
    Value::Str(Text::from(text))
}

/// `needle in v` for a string needle, or the `TypeError` at `line`.
pub fn py_in(needle: &str, v: &Value, line: u32) -> Result<bool, Raise> {
    match v {
        Value::Str(_) => Ok(py::str_of(v).contains(needle)),
        Value::Array(items) => Ok(items.iter().any(|x| *x == s(needle))),
        Value::Object(pairs) => Ok(pairs.iter().any(|(k, _)| *k == Text::from(needle))),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: argument of type '{}' is not iterable",
                py::type_name(other)
            ),
        )),
    }
}

/// `v + "<str>"`: the text, or the `TypeError` at `line`.
pub fn plus_str(v: &Value, line: u32) -> Result<String, Raise> {
    match v {
        Value::Str(_) => Ok(py::str_of(v)),
        Value::Array(_) => Err(Raise::at(
            line,
            "TypeError: can only concatenate list (not \"str\") to list",
        )),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: unsupported operand type(s) for +: '{}' and 'str'",
                py::type_name(other)
            ),
        )),
    }
}

/// `"<str>" + v`: the text, or the `TypeError` at `line`.
pub fn str_plus(v: &Value, line: u32) -> Result<String, Raise> {
    match v {
        Value::Str(_) => Ok(py::str_of(v)),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: can only concatenate str (not \"{}\") to str",
                py::type_name(other)
            ),
        )),
    }
}

/// `sep.join(v)`, or the `TypeError` at `line`.
pub fn join(sep: &str, v: &Value, line: u32) -> Result<String, Raise> {
    let items: Vec<Value> = match v {
        Value::Array(items) => items.clone(),
        Value::Str(t) => t
            .code_points()
            .into_iter()
            .map(|cp| Value::Str(super::super::code_point(cp)))
            .collect(),
        Value::Object(pairs) => pairs.iter().map(|(k, _)| Value::Str(k.clone())).collect(),
        _ => {
            return Err(Raise::at(line, "TypeError: can only join an iterable"));
        }
    };
    let mut parts = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        match item {
            Value::Str(_) => parts.push(py::str_of(item)),
            other => {
                return Err(Raise::at(
                    line,
                    format!(
                        "TypeError: sequence item {i}: expected str instance, {} found",
                        py::type_name(other)
                    ),
                ))
            }
        }
    }
    Ok(parts.join(sep))
}

/// `v.get(...)` on a value that must be a dict, or the `AttributeError`.
pub fn dict(v: &Value, line: u32) -> Result<&Value, Raise> {
    match v {
        Value::Object(_) => Ok(v),
        other => Err(Raise::no_attribute(line, other, "get")),
    }
}

/// `json.load(sys.stdin)` / `json.loads(raw)` at `line`: the value, or the
/// `JSONDecodeError` (its wording is ours past the class name).
pub fn load(raw: &str, line: u32) -> Result<Value, Raise> {
    json::parse(raw).map_err(|e| {
        let why = if raw.trim_matches(py_space).is_empty() {
            "Expecting value: line 1 column 1 (char 0)".to_string()
        } else {
            e.to_string()
        };
        Raise::at(line, format!("json.decoder.JSONDecodeError: {why}"))
    })
}

/// `s.strip()`.
pub fn strip(text: &str) -> &str {
    text.trim_matches(py_space)
}

/// `[x.strip() for x in text.split(",") if x.strip()]`.
pub fn comma_list(text: &str) -> Vec<Value> {
    text.split(',')
        .map(strip)
        .filter(|x| !x.is_empty())
        .map(s)
        .collect()
}

/// `text.splitlines()`: at `\n`, `\r`, `\r\n`, `\v`, `\f`, `\x1c`, `\x1d`,
/// `\x1e`, `\x85`, U+2028 and U+2029; no empty last line after a final
/// break.
pub fn splitlines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if matches!(
            c,
            '\n' | '\r'
                | '\u{0b}'
                | '\u{0c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        ) {
            out.push(&text[start..i]);
            let mut next = i + c.len_utf8();
            if c == '\r' {
                if let Some((j, '\n')) = chars.peek().copied() {
                    chars.next();
                    next = j + 1;
                }
            }
            start = next;
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// `[x.strip() for x in text.splitlines() if x.strip()]`.
pub fn line_list(text: &str) -> Vec<Value> {
    splitlines(text)
        .into_iter()
        .map(strip)
        .filter(|x| !x.is_empty())
        .map(s)
        .collect()
}

/// `json.dumps(v)` with Python's defaults.
pub fn dumps(v: &Value) -> String {
    json::to_string(v, &json::Options::default()).unwrap_or_default()
}

/// A dict from its pairs, in order.
pub fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (Text::from(k), v)).collect())
}

/// `raw[:n]` by code points.
pub fn head(raw: &str, n: usize) -> &str {
    match raw.char_indices().nth(n) {
        Some((i, _)) => &raw[..i],
        None => raw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitlines_as_python() {
        assert_eq!(splitlines("a\nb\n"), vec!["a", "b"]);
        assert_eq!(splitlines("a\r\nb\rc\u{2028}d"), vec!["a", "b", "c", "d"]);
        assert_eq!(splitlines("\n\n"), vec!["", ""]);
        assert_eq!(splitlines(""), Vec::<&str>::new());
    }

    #[test]
    fn comma_and_line_lists() {
        assert_eq!(comma_list(" a, ,b ,\u{a0}"), vec![s("a"), s("b")]);
        assert_eq!(line_list(" x \n\ny\r\n"), vec![s("x"), s("y")]);
    }

    #[test]
    fn join_raises_as_python() {
        assert_eq!(join(", ", &json::parse(r#"["a","b"]"#).unwrap(), 1).unwrap(), "a, b");
        assert_eq!(join(",", &s("ab"), 1).unwrap(), "a,b");
        assert_eq!(
            join(",", &json::parse("[1]").unwrap(), 7).unwrap_err(),
            Raise::at(7, "TypeError: sequence item 0: expected str instance, int found")
        );
        assert_eq!(
            join(",", &Value::Bool(true), 7).unwrap_err(),
            Raise::at(7, "TypeError: can only join an iterable")
        );
    }

    #[test]
    fn head_counts_code_points() {
        assert_eq!(head("é".repeat(3).as_str(), 2), "éé");
        assert_eq!(head("ab", 5), "ab");
    }
}
