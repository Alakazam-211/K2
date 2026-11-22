//! Helpers more than one family uses: a POST on the left of a pipe, and
//! the few Python operations the shell's readers raise in. Lifted from
//! [`crate::families::organization`] and [`super`] (remind), which had
//! near copies; workspace, review, skills, whats-new and commit use them
//! too. It lives here, in a family's module, until it gets
//! a place of its own (`families/mod.rs` is S0's).
//!
//! Each Python helper takes `at`, which makes the [`Raise`] for an
//! exception at the reader's line ([`at_line`] for a line of the module).
//! The ones from [`decode_error`] on give `str(e)` instead, for readers
//! that print `"Error: %s" % e`.

use crate::exit::Exit;
use crate::families::{end_in_pipe, get, piped_body, py_iter, Piped, Raise};
use crate::out::json::{self, Value};
use crate::out::{self, py};
use crate::prelude::Closing;
use crate::transport::{Call, Client, Error};

type R<T> = Result<T, Raise>;

/// `cli_send cli_request_post <endpoint> <args>` on the left of a pipe:
/// as [`crate::families::request_piped`], a POST. One that went out on the cell
/// socket and may have been applied is `_cli_send_unanswered`'s line,
/// then the end of the command, as a request with no answer.
pub fn post_piped(
    client: &mut Client,
    closing: &mut Closing,
    endpoint: &str,
    args: &[&str],
) -> Piped {
    let passport = client.passport().is_some();
    match client.send(endpoint, Call::RequestPost(args)) {
        Ok(r) => {
            closing.answer(&r, passport);
            Piped {
                body: piped_body(&r),
                exit: Exit::Ok,
            }
        }
        Err(e @ (Error::NoAnswer(_) | Error::MaybeApplied { .. })) => {
            out::stderr_line(&e.to_string());
            end_in_pipe()
        }
        Err(e) => Piped {
            body: Vec::new(),
            exit: Exit::from_curl(e.curl_code()),
        },
    }
}

/// An exception at `line` of the module.
pub fn at_line(line: u32) -> impl Fn(String) -> Raise {
    move |e| Raise::at(line, e)
}

/// `v[key]` with a string key.
pub fn sub<'a>(v: &'a Value, key: &str, at: &dyn Fn(String) -> Raise) -> R<&'a Value> {
    match v {
        Value::Object(pairs) => get(pairs, key)
            .ok_or_else(|| at(format!("KeyError: {}", py::repr(&crate::families::s(key))))),
        Value::Array(_) => Err(at(
            "TypeError: list indices must be integers or slices, not str".into(),
        )),
        Value::Str(_) => Err(at(
            "TypeError: string indices must be integers, not 'str'".into()
        )),
        other => Err(at(format!(
            "TypeError: '{}' object is not subscriptable",
            py::type_name(other)
        ))),
    }
}

/// `v.get(key)`: `None` for a missing key, the `AttributeError` for
/// anything but a dict.
pub fn dget<'a>(v: &'a Value, key: &str, at: &dyn Fn(String) -> Raise) -> R<Option<&'a Value>> {
    match v {
        Value::Object(pairs) => Ok(get(pairs, key)),
        other => Err(at(format!(
            "AttributeError: '{}' object has no attribute 'get'",
            py::type_name(other)
        ))),
    }
}

/// `v + "<tail>"` where `v` must be a string.
pub fn plus_str(v: &Value, tail: &str, at: &dyn Fn(String) -> Raise) -> R<String> {
    match v {
        Value::Str(_) => Ok(py::str_of(v) + tail),
        Value::Array(_) => Err(at(
            "TypeError: can only concatenate list (not \"str\") to list".into(),
        )),
        other => Err(at(format!(
            "TypeError: unsupported operand type(s) for +: '{}' and 'str'",
            py::type_name(other)
        ))),
    }
}

/// `", ".join(v)`.
pub fn join(v: &Value, at: &dyn Fn(String) -> Raise) -> R<String> {
    let items = match v {
        Value::Array(_) | Value::Str(_) | Value::Object(_) => {
            py_iter(v, 0).map_err(|r| at(r.exception))?
        }
        _ => return Err(at("TypeError: can only join an iterable".into())),
    };
    join_items(&items, at)
}

/// `", ".join(items)` on items already walked.
pub fn join_items(items: &[Value], at: &dyn Fn(String) -> Raise) -> R<String> {
    let mut parts = Vec::new();
    for (i, item) in items.iter().enumerate() {
        match item {
            Value::Str(_) => parts.push(py::str_of(item)),
            other => {
                return Err(at(format!(
                    "TypeError: sequence item {i}: expected str instance, {} found",
                    py::type_name(other)
                )))
            }
        }
    }
    Ok(parts.join(", "))
}

/// `x or default`.
pub fn or(v: Option<&Value>, default: Value) -> Value {
    v.filter(|x| py::truthy(x)).cloned().unwrap_or(default)
}

/// `str(e)` for what `json.loads(raw)` raises: Python's message, with the
/// line, column and character where it stopped.
pub fn decode_error(raw: &str) -> String {
    let (what, at) = match json::parse(raw) {
        Ok(_) => return String::new(),
        Err(json::Error::Syntax { what, at }) => (what, at),
        Err(json::Error::TooDeep { at } | json::Error::IntTooLong { at }) => {
            ("expecting a value", at)
        }
        Err(json::Error::LoneSurrogate { .. }) => ("expecting a value", 0),
    };
    let msg = match what {
        "extra data after the value" => "Extra data",
        "expecting ',' or ']'" | "expecting ',' or '}'" => "Expecting ',' delimiter",
        "expecting a key in double quotes" => "Expecting property name enclosed in double quotes",
        "expecting ':'" => "Expecting ':' delimiter",
        "control character inside a string" => "Invalid control character at",
        "invalid \\ escape" => "Invalid \\escape",
        "invalid \\uXXXX escape" => "Invalid \\uXXXX escape",
        "string without a closing quote" => "Unterminated string starting at",
        _ => "Expecting value",
    };
    let at = at.min(raw.len());
    let before = raw.get(..at).unwrap_or(raw);
    let pos = before.chars().count();
    let line = before.matches('\n').count() + 1;
    let col = match before.rfind('\n') {
        Some(i) => before[i + 1..].chars().count() + 1,
        None => pos + 1,
    };
    format!("{msg}: line {line} column {col} (char {pos})")
}

/// `str(e)` for what `"<key>" in d` raises, or whether it holds.
pub fn py_in(key: &str, d: &Value) -> Result<bool, String> {
    match d {
        Value::Object(pairs) => Ok(get(pairs, key).is_some()),
        Value::Array(items) => Ok(items.contains(&crate::families::s(key))),
        Value::Str(_) => Ok(py::str_of(d).contains(key)),
        other => Err(format!(
            "argument of type '{}' is not iterable",
            py::type_name(other)
        )),
    }
}

/// `str(e)` for `d[key]`, or the value.
pub fn py_index<'a>(d: &'a Value, key: &str) -> Result<&'a Value, String> {
    match d {
        Value::Object(pairs) => get(pairs, key).ok_or_else(|| py::repr(&crate::families::s(key))),
        Value::Array(_) => Err("list indices must be integers or slices, not str".into()),
        Value::Str(_) => Err("string indices must be integers, not 'str'".into()),
        other => Err(format!(
            "'{}' object is not subscriptable",
            py::type_name(other)
        )),
    }
}

/// `str(e)` for `d.get(key)`, or the value (`None` for a missing key).
pub fn py_get<'a>(d: &'a Value, key: &str) -> Result<Option<&'a Value>, String> {
    match d {
        Value::Object(pairs) => Ok(get(pairs, key)),
        other => Err(format!(
            "'{}' object has no attribute 'get'",
            py::type_name(other)
        )),
    }
}

/// `"<head>" + v` where `v` must be a string.
pub fn str_plus(head: &str, v: &Value) -> Result<String, String> {
    match v {
        Value::Str(_) => Ok(format!("{head}{}", py::str_of(v))),
        other => Err(format!(
            "can only concatenate str (not \"{}\") to str",
            py::type_name(other)
        )),
    }
}
