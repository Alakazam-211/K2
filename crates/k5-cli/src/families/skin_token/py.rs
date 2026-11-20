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
/// `JSONDecodeError` with Python 3.12's message ([`decode_error`]).
pub fn load(raw: &str, line: u32) -> Result<Value, Raise> {
    json::parse(raw).map_err(|e| {
        let why = decode_error(raw).unwrap_or_else(|| e.to_string());
        Raise::at(line, format!("json.decoder.JSONDecodeError: {why}"))
    })
}

/// `str(e)` of the `JSONDecodeError` Python 3.12's `json.loads(raw)`
/// raises (its C scanner's messages and positions), or `None` when Python
/// reads it. Positions count code points.
pub fn decode_error(raw: &str) -> Option<String> {
    let s: Vec<char> = raw.chars().collect();
    let (msg, pos) = if s.first() == Some(&'\u{feff}') {
        ("Unexpected UTF-8 BOM (decode using utf-8-sig)", 0)
    } else {
        let start = ws(&s, 0);
        match scan(&s, start, 0) {
            Err(e) => e,
            Ok(end) => {
                let end = ws(&s, end);
                if end == s.len() {
                    return None;
                }
                ("Extra data", end)
            }
        }
    };
    let before = &s[..pos.min(s.len())];
    let line = before.iter().filter(|c| **c == '\n').count() + 1;
    let col = match before.iter().rposition(|c| *c == '\n') {
        Some(nl) => pos - nl,
        None => pos + 1,
    };
    Some(format!("{msg}: line {line} column {col} (char {pos})"))
}

type Scan = Result<usize, (&'static str, usize)>;

fn ws(s: &[char], mut i: usize) -> usize {
    while i < s.len() && matches!(s[i], ' ' | '\t' | '\n' | '\r') {
        i += 1;
    }
    i
}

fn starts(s: &[char], i: usize, word: &str) -> bool {
    let w: Vec<char> = word.chars().collect();
    s.len() >= i + w.len() && s[i..i + w.len()] == w[..]
}

/// `scan_once` at `i`: the end of the value, or the error and its place.
fn scan(s: &[char], i: usize, depth: usize) -> Scan {
    const EXPECT: &str = "Expecting value";
    if i >= s.len() || depth > json::MAX_DEPTH {
        return Err((EXPECT, i));
    }
    match s[i] {
        '"' => string(s, i + 1),
        '{' => object(s, i + 1, depth + 1),
        '[' => array(s, i + 1, depth + 1),
        'n' if starts(s, i, "null") => Ok(i + 4),
        't' if starts(s, i, "true") => Ok(i + 4),
        'f' if starts(s, i, "false") => Ok(i + 5),
        'N' if starts(s, i, "NaN") => Ok(i + 3),
        'I' if starts(s, i, "Infinity") => Ok(i + 8),
        '-' if starts(s, i, "-Infinity") => Ok(i + 9),
        _ => number(s, i).ok_or((EXPECT, i)),
    }
}

fn number(s: &[char], start: usize) -> Option<usize> {
    let digit = |i: usize| i < s.len() && s[i].is_ascii_digit();
    let mut i = start;
    if s[i] == '-' {
        i += 1;
    }
    match s.get(i) {
        Some('1'..='9') => {
            while digit(i) {
                i += 1;
            }
        }
        Some('0') => i += 1,
        _ => return None,
    }
    if i + 1 < s.len() && s[i] == '.' && digit(i + 1) {
        i += 1;
        while digit(i) {
            i += 1;
        }
    }
    if i + 1 < s.len() && matches!(s[i], 'e' | 'E') {
        let mut j = i + 1;
        if j < s.len() && matches!(s[j], '+' | '-') {
            j += 1;
        }
        if digit(j) {
            while digit(j) {
                j += 1;
            }
            i = j;
        }
    }
    Some(i)
}

fn string(s: &[char], mut end: usize) -> Scan {
    let begin = end - 1;
    const OPEN: &str = "Unterminated string starting at";
    const UXXXX: &str = "Invalid \\uXXXX escape";
    let hex4 = |from: usize| -> Result<u32, (&'static str, usize)> {
        if from + 4 >= s.len() {
            return Err((UXXXX, from - 1));
        }
        let text: String = s[from..from + 4].iter().collect();
        if !text.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err((UXXXX, from - 1));
        }
        Ok(u32::from_str_radix(&text, 16).unwrap_or(0))
    };
    loop {
        let mut next = end;
        while next < s.len() && s[next] != '"' && s[next] != '\\' {
            if (s[next] as u32) <= 0x1f {
                return Err(("Invalid control character at", next));
            }
            next += 1;
        }
        if next >= s.len() {
            return Err((OPEN, begin));
        }
        if s[next] == '"' {
            return Ok(next + 1);
        }
        next += 1;
        if next == s.len() {
            return Err((OPEN, begin));
        }
        if s[next] != 'u' {
            if !matches!(s[next], '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't') {
                return Err(("Invalid \\escape", next - 1));
            }
            end = next + 1;
            continue;
        }
        let cp = hex4(next + 1)?;
        end = next + 5;
        if (0xd800..=0xdbff).contains(&cp)
            && end + 1 < s.len()
            && s[end] == '\\'
            && s[end + 1] == 'u'
        {
            let low = hex4(end + 2)?;
            if (0xdc00..=0xdfff).contains(&low) {
                end += 6;
            }
        }
    }
}

fn object(s: &[char], i: usize, depth: usize) -> Scan {
    let mut i = ws(s, i);
    if i < s.len() && s[i] == '}' {
        return Ok(i + 1);
    }
    loop {
        if i >= s.len() || s[i] != '"' {
            return Err(("Expecting property name enclosed in double quotes", i));
        }
        i = ws(s, string(s, i + 1)?);
        if i >= s.len() || s[i] != ':' {
            return Err(("Expecting ':' delimiter", i));
        }
        i = ws(s, scan(s, ws(s, i + 1), depth)?);
        if i < s.len() && s[i] == '}' {
            return Ok(i + 1);
        }
        if i >= s.len() || s[i] != ',' {
            return Err(("Expecting ',' delimiter", i));
        }
        i = ws(s, i + 1);
    }
}

fn array(s: &[char], i: usize, depth: usize) -> Scan {
    let mut i = ws(s, i);
    if i < s.len() && s[i] == ']' {
        return Ok(i + 1);
    }
    loop {
        i = ws(s, scan(s, i, depth)?);
        if i < s.len() && s[i] == ']' {
            return Ok(i + 1);
        }
        if i >= s.len() || s[i] != ',' {
            return Err(("Expecting ',' delimiter", i));
        }
        i = ws(s, i + 1);
    }
}

/// `str(e)` of a Python exception this module raised: its text after the
/// class name (`AttributeError: x` is `x`).
pub fn message(r: &Raise) -> String {
    match r.exception.split_once(": ") {
        Some((_, m)) => m.to_string(),
        None => String::new(),
    }
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
    fn decode_errors_as_python_3_12() {
        for (raw, want) in [
            ("", "Expecting value: line 1 column 1 (char 0)"),
            ("  \n ", "Expecting value: line 2 column 2 (char 4)"),
            ("<html>", "Expecting value: line 1 column 1 (char 0)"),
            ("{\"a\":1} x", "Extra data: line 1 column 9 (char 8)"),
            ("{\"a\":1,}", "Expecting property name enclosed in double quotes: line 1 column 8 (char 7)"),
            ("{\"a\" 1}", "Expecting ':' delimiter: line 1 column 6 (char 5)"),
            ("{\"a\":1 \"b\"}", "Expecting ',' delimiter: line 1 column 8 (char 7)"),
            ("[1,]", "Expecting value: line 1 column 4 (char 3)"),
            ("[1 2]", "Expecting ',' delimiter: line 1 column 4 (char 3)"),
            ("\"ab", "Unterminated string starting at: line 1 column 1 (char 0)"),
            ("\"a\tb\"", "Invalid control character at: line 1 column 3 (char 2)"),
            ("\"a\\qb\"", "Invalid \\escape: line 1 column 3 (char 2)"),
            ("\"\\u12\"", "Invalid \\uXXXX escape: line 1 column 3 (char 2)"),
            ("01", "Extra data: line 1 column 2 (char 1)"),
            ("-", "Expecting value: line 1 column 1 (char 0)"),
            ("1.", "Extra data: line 1 column 2 (char 1)"),
            ("\u{feff}{}", "Unexpected UTF-8 BOM (decode using utf-8-sig): line 1 column 1 (char 0)"),
        ] {
            assert_eq!(decode_error(raw).as_deref(), Some(want), "{raw:?}");
        }
        for ok in ["{}", " [1, -2.5e3, NaN, -Infinity, \"\\ud83d\\ude00\"] ", "null"] {
            assert_eq!(decode_error(ok), None, "{ok:?}");
        }
    }

    #[test]
    fn head_counts_code_points() {
        assert_eq!(head("é".repeat(3).as_str(), 2), "éé");
        assert_eq!(head("ab", 5), "ab");
    }
}
