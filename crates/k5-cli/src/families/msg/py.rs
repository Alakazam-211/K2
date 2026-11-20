//! What the shell's Python readers for `msg who` and `msg verify` do with
//! the values they meet: Python's exceptions and its `==`, and the
//! error branch both share.
//!
//! The readers run with Python's UTF-8 mode (the C locale of a CLI run
//! with no `LANG`): a byte that is not UTF-8 is read as a stand-in
//! (`surrogateescape`) and written to stderr as `\udcXX`
//! (`backslashreplace`). Such an answer is never JSON here, so it reaches
//! only the "daemon answered" line.

use super::super::{get, Raise};
use crate::out::json::{self, Text, Value};
use crate::out::py::{str_of, truthy, type_name};

/// What the reader printed so far, and how it ends.
#[derive(Debug, Default)]
pub struct Printed {
    pub stdout: String,
    pub stderr: String,
}

impl Printed {
    /// `print(line)`.
    pub fn line(&mut self, line: &str) {
        self.stdout.push_str(line);
        self.stdout.push('\n');
    }

    /// `print(line, file=sys.stderr)`.
    pub fn err(&mut self, line: &str) {
        self.stderr.push_str(line);
        self.stderr.push('\n');
    }
}

/// One code point of what Python read: a character, or a byte that was
/// not UTF-8.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    Char(char),
    Byte(u8),
}

fn units(raw: &[u8]) -> Vec<Unit> {
    let mut out = Vec::new();
    let mut rest = raw;
    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(s) => {
                out.extend(s.chars().map(Unit::Char));
                break;
            }
            Err(e) => {
                let good = e.valid_up_to();
                let s = std::str::from_utf8(&rest[..good]).unwrap_or_default();
                out.extend(s.chars().map(Unit::Char));
                let bad = e.error_len().unwrap_or(rest.len() - good);
                out.extend(rest[good..good + bad].iter().map(|b| Unit::Byte(*b)));
                rest = &rest[good + bad..];
            }
        }
    }
    out
}

/// `"Error: daemon answered: %s" % raw[:300]` as Python writes it to
/// stderr.
pub fn daemon_answered(raw: &[u8]) -> String {
    let head: String = units(raw)
        .into_iter()
        .take(300)
        .map(|u| match u {
            Unit::Char(c) => c.to_string(),
            Unit::Byte(b) => format!("\\udc{b:02x}"),
        })
        .collect();
    format!("Error: daemon answered: {head}")
}

/// `json.loads(sys.stdin.read())`, or `None` where it raises.
pub fn loads(raw: &[u8]) -> Option<Value> {
    json::parse(std::str::from_utf8(raw).ok()?).ok()
}

/// `v.get(key)`: `None` for a missing key, the `AttributeError` at `line`
/// for anything but a dict.
pub fn get_at<'a>(v: &'a Value, key: &str, line: u32) -> Result<Option<&'a Value>, Raise> {
    match v {
        Value::Object(pairs) => Ok(get(pairs, key)),
        other => Err(Raise::at(
            line,
            format!(
                "AttributeError: '{}' object has no attribute 'get'",
                type_name(other)
            ),
        )),
    }
}

/// `v.get(key)` as a value: `None` is JSON's null.
pub fn get_or_null(v: &Value, key: &str, line: u32) -> Result<Value, Raise> {
    Ok(get_at(v, key, line)?.cloned().unwrap_or(Value::Null))
}

/// `v[key]`, or the exception Python raises at `line`.
pub fn index<'a>(v: &'a Value, key: &str, line: u32) -> Result<&'a Value, Raise> {
    match v {
        Value::Object(pairs) => {
            get(pairs, key).ok_or_else(|| Raise::at(line, format!("KeyError: {}", key_repr(key))))
        }
        Value::Str(_) => Err(Raise::at(
            line,
            "TypeError: string indices must be integers, not 'str'",
        )),
        Value::Array(_) => Err(Raise::at(
            line,
            "TypeError: list indices must be integers or slices, not str",
        )),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: '{}' object is not subscriptable",
                type_name(other)
            ),
        )),
    }
}

fn key_repr(key: &str) -> String {
    crate::out::py::repr(&Value::Str(Text::from(key)))
}

/// `a or b`.
pub fn or(a: Value, b: Value) -> Value {
    if truthy(&a) {
        a
    } else {
        b
    }
}

/// `left + v` for a string `left`, or the `TypeError` at `line`.
pub fn concat(left: &str, v: &Value, line: u32) -> Result<String, Raise> {
    match v {
        Value::Str(_) => Ok(format!("{left}{}", str_of(v))),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: can only concatenate str (not \"{}\") to str",
                type_name(other)
            ),
        )),
    }
}

/// `v + ": "` for a value that is true, or the `TypeError` at `line`.
fn plus_colon(v: &Value, line: u32) -> Result<String, Raise> {
    match v {
        Value::Str(_) => Ok(format!("{}: ", str_of(v))),
        Value::Array(_) => Err(Raise::at(
            line,
            "TypeError: can only concatenate list (not \"str\") to list",
        )),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: unsupported operand type(s) for +: '{}' and 'str'",
                type_name(other)
            ),
        )),
    }
}

/// Python's `a == b` for two values `json.loads` gave.
pub fn eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Str(x), Value::Str(y)) => x.code_points() == y.code_points(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| eq(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.iter().any(|(k2, v2)| k == k2 && eq(v, v2)))
        }
        _ => match (number(a), number(b)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        },
    }
}

/// A number as Python compares it: an exact integer, or a float.
#[derive(Debug, Clone, Copy)]
enum Num {
    Int(i128),
    Float(f64),
}

impl PartialEq for Num {
    fn eq(&self, other: &Num) -> bool {
        match (*self, *other) {
            (Num::Int(a), Num::Int(b)) => a == b,
            (Num::Float(a), Num::Float(b)) => a == b,
            (Num::Int(i), Num::Float(f)) | (Num::Float(f), Num::Int(i)) => {
                f.is_finite() && f.fract() == 0.0 && (f as i128) == i && (i as f64) == f
            }
        }
    }
}

fn number(v: &Value) -> Option<Num> {
    match v {
        Value::Bool(b) => Some(Num::Int(i128::from(*b))),
        Value::Int(d) => d.parse().ok().map(Num::Int),
        Value::Float(f) => Some(Num::Float(*f)),
        _ => None,
    }
}

/// The readers' shared error branch (`_hire_print` and `msg verify`'s),
/// lines 8 to 15: `Some(exit)` when `d` holds a true `error`, with its
/// line printed. `exits` maps a code to its exit (anything else is 1);
/// `refused`: `_hire_print`'s exit 3 for a refusing code or a hint naming
/// a token (line 13), before the map (line 15).
pub fn error_branch(
    d: &Value,
    p: &mut Printed,
    exits: &[(&str, i32)],
    refused: bool,
) -> Result<Option<i32>, Raise> {
    let lines = if refused { (12, 15) } else { (12, 13) };
    error_branch_at(d, p, exits, refused, lines)
}

/// [`error_branch`] for a reader whose lines are elsewhere: `lines` is
/// the line of the `Error:` print (`code + ": "`) and the line of the map
/// (`unhashable type`).
pub fn error_branch_at(
    d: &Value,
    p: &mut Printed,
    exits: &[(&str, i32)],
    refused: bool,
    lines: (u32, u32),
) -> Result<Option<i32>, Raise> {
    let err = match d {
        Value::Object(pairs) => get(pairs, "error").cloned().unwrap_or(Value::Null),
        _ => Value::Null,
    };
    if !truthy(&err) {
        return Ok(None);
    }
    let (code, hint) = match &err {
        Value::Object(pairs) => (
            get(pairs, "code").cloned().unwrap_or_else(|| s("")),
            get(pairs, "hint").cloned().unwrap_or_else(|| s("")),
        ),
        other => (s(""), s(&str_of(other))),
    };
    let prefix = if truthy(&code) {
        plus_colon(&code, lines.0)?
    } else {
        String::new()
    };
    p.err(&format!("Error: {prefix}{}", str_of(&hint)));
    if refused {
        let refusing = ["owner_only", "forbidden", "exceeds_key", "role_required"]
            .iter()
            .any(|c| eq(&code, &s(c)));
        if refusing {
            return Ok(Some(3));
        }
        let Value::Str(h) = &hint else {
            return Err(Raise::at(
                13,
                format!(
                    "AttributeError: '{}' object has no attribute 'lower'",
                    type_name(&hint)
                ),
            ));
        };
        if str_of(&Value::Str(h.clone()))
            .to_lowercase()
            .contains("token")
        {
            return Ok(Some(3));
        }
    }
    if matches!(code, Value::Array(_) | Value::Object(_)) {
        return Err(Raise::at(
            lines.1,
            format!("TypeError: unhashable type: '{}'", type_name(&code)),
        ));
    }
    let exit = exits
        .iter()
        .find(|(c, _)| eq(&code, &s(c)))
        .map_or(1, |(_, e)| *e);
    Ok(Some(exit))
}

/// A string value.
pub fn s(text: &str) -> Value {
    Value::Str(Text::from(text))
}

/// `json.dumps(v, indent=2)`.
pub fn dumps_indent(v: &Value) -> String {
    json::to_string(
        v,
        &json::Options {
            indent: Some(2),
            ..json::Options::default()
        },
    )
    .unwrap_or_default()
}

/// `json.dumps(v)`.
pub fn dumps(v: &Value) -> String {
    json::to_string(v, &json::Options::default()).unwrap_or_default()
}
