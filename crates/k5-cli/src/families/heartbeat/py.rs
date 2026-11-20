//! The Python the heartbeat readers run on a value `json.loads` gave, with
//! the exception Python raises where the value is not what they expect.
//! An `Err` is the exception as `Class: message` ([`msg`] is `str(e)`).

use crate::exit::Exit;
use crate::families::remind::shared::decode_error;
use crate::families::{get, py_iter, s, Raise, Shown};
use crate::out::json::{self, Value};
use crate::out::py;

pub type R<T> = Result<T, String>;

/// `str(e)` of an exception `Class: message`.
pub fn msg(e: &str) -> &str {
    e.split_once(": ").map_or(e, |(_, m)| m)
}

/// What a reader printed so far.
#[derive(Default)]
pub struct Py {
    pub out: String,
    pub err: String,
}

impl Py {
    pub fn out(&mut self, line: impl AsRef<str>) {
        self.out.push_str(line.as_ref());
        self.out.push('\n');
    }

    pub fn err(&mut self, line: impl AsRef<str>) {
        self.err.push_str(line.as_ref());
        self.err.push('\n');
    }

    /// The reader's end: `except Exception as e: print(f'Error: {e}')`
    /// and exit 1 for an exception.
    pub fn caught(mut self, r: R<Exit>) -> Shown {
        let exit = match r {
            Ok(exit) => exit,
            Err(e) => {
                self.err(format!("Error: {}", msg(&e)));
                Exit::Error
            }
        };
        Shown {
            stdout: self.out,
            stderr: self.err,
            exit,
        }
    }

    /// The reader's end where nothing catches: the traceback, exit 1.
    pub fn raised(mut self, r: R<Exit>, line: u32) -> Shown {
        let exit = match r {
            Ok(exit) => exit,
            Err(e) => {
                self.err.push_str(&Raise::at(line, e).text());
                Exit::Error
            }
        };
        Shown {
            stdout: self.out,
            stderr: self.err,
            exit,
        }
    }
}

/// `json.loads(raw)`.
pub fn loads(raw: &str) -> R<Value> {
    json::parse(raw).map_err(|_| format!("json.decoder.JSONDecodeError: {}", decode_error(raw)))
}

/// `json.loads(v)` on a value.
pub fn loads_value(v: &Value) -> R<Value> {
    match v {
        Value::Str(t) => loads(&py::str_of(&Value::Str(t.clone()))),
        other => Err(format!(
            "TypeError: the JSON object must be str, bytes or bytearray, not {}",
            py::type_name(other)
        )),
    }
}

/// `v.get(key)`.
pub fn dget<'a>(v: &'a Value, key: &str) -> R<Option<&'a Value>> {
    match v {
        Value::Object(pairs) => Ok(get(pairs, key)),
        other => Err(format!(
            "AttributeError: '{}' object has no attribute 'get'",
            py::type_name(other)
        )),
    }
}

/// `v.get(key, default)`.
pub fn dget_or(v: &Value, key: &str, default: Value) -> R<Value> {
    Ok(dget(v, key)?.cloned().unwrap_or(default))
}

/// `v.get(key)`, `None` as JSON null.
pub fn dget_v(v: &Value, key: &str) -> R<Value> {
    dget_or(v, key, Value::Null)
}

/// `x or default`.
pub fn or(x: Value, default: Value) -> Value {
    if py::truthy(&x) {
        x
    } else {
        default
    }
}

/// `x == "<word>"`.
pub fn is(x: &Value, word: &str) -> bool {
    *x == s(word)
}

/// `key in v`.
pub fn has(v: &Value, key: &str) -> R<bool> {
    crate::families::remind::shared::py_in(key, v).map_err(|m| format!("TypeError: {m}"))
}

/// `v[key]`.
pub fn index(v: &Value, key: &str) -> R<Value> {
    match crate::families::remind::shared::py_index(v, key) {
        Ok(x) => Ok(x.clone()),
        Err(m) if matches!(v, Value::Object(_)) => Err(format!("KeyError: {m}")),
        Err(m) => Err(format!("TypeError: {m}")),
    }
}

/// `len(v)`.
pub fn len(v: &Value) -> R<usize> {
    crate::families::py_len(v, 0).map_err(|r| r.exception)
}

/// `for x in v`.
pub fn iter(v: &Value) -> R<Vec<Value>> {
    py_iter(v, 0).map_err(|r| r.exception)
}

/// `sep.join(v)`.
pub fn join(v: &Value, sep: &str) -> R<String> {
    let items = match v {
        Value::Array(_) | Value::Str(_) | Value::Object(_) => iter(v)?,
        _ => return Err("TypeError: can only join an iterable".into()),
    };
    let mut parts = Vec::new();
    for (i, item) in items.iter().enumerate() {
        match item {
            Value::Str(_) => parts.push(py::str_of(item)),
            other => {
                return Err(format!(
                    "TypeError: sequence item {i}: expected str instance, {} found",
                    py::type_name(other)
                ))
            }
        }
    }
    Ok(parts.join(sep))
}

/// `f'{v:<width}'`.
pub fn left(v: &Value, width: usize) -> R<String> {
    let text = match v {
        Value::Str(_) | Value::Int(_) | Value::Float(_) => py::str_of(v),
        Value::Bool(b) => if *b { "1" } else { "0" }.to_string(),
        other => {
            return Err(format!(
                "TypeError: unsupported format string passed to {}.__format__",
                py::type_name(other)
            ))
        }
    };
    Ok(crate::families::ljust(&text, width))
}

/// `a + b` where `b` is a string.
pub fn plus(a: &Value, b: &str) -> R<String> {
    match a {
        Value::Str(_) => Ok(py::str_of(a) + b),
        Value::Array(_) => Err("TypeError: can only concatenate list (not \"str\") to list".into()),
        other => Err(format!(
            "TypeError: unsupported operand type(s) for +: '{}' and 'str'",
            py::type_name(other)
        )),
    }
}

/// `"<a>" + v` where `v` must be a string.
pub fn str_plus(a: &str, v: &Value) -> R<String> {
    match v {
        Value::Str(_) => Ok(format!("{a}{}", py::str_of(v))),
        other => Err(format!(
            "TypeError: can only concatenate str (not \"{}\") to str",
            py::type_name(other)
        )),
    }
}

/// `v[:19].replace('T', ' ')`.
pub fn stamp(v: &Value) -> R<String> {
    match v {
        Value::Str(_) => Ok(py::str_of(v)
            .chars()
            .take(19)
            .collect::<String>()
            .replace('T', " ")),
        Value::Array(_) => Err("AttributeError: 'list' object has no attribute 'replace'".into()),
        Value::Object(_) => Err("TypeError: unhashable type: 'slice'".into()),
        other => Err(format!(
            "TypeError: '{}' object is not subscriptable",
            py::type_name(other)
        )),
    }
}

/// `v.strip()` (only the emptiness matters to the readers).
pub fn stripped(v: &Value) -> R<String> {
    match v {
        Value::Str(_) => Ok(py::str_of(v)
            .trim_matches(crate::families::py_space)
            .to_string()),
        other => Err(format!(
            "AttributeError: '{}' object has no attribute 'strip'",
            py::type_name(other)
        )),
    }
}

/// `int(v)`.
pub fn int(v: &Value) -> R<i128> {
    let bad = || -> String {
        format!(
            "ValueError: invalid literal for int() with base 10: {}",
            py::repr(v)
        )
    };
    match v {
        Value::Int(d) => d.parse().map_err(|_| "OverflowError: int too large".to_string()),
        Value::Bool(b) => Ok(i128::from(*b)),
        Value::Float(f) if f.is_nan() => {
            Err("ValueError: cannot convert float NaN to integer".into())
        }
        Value::Float(f) if f.is_infinite() => {
            Err("OverflowError: cannot convert float infinity to integer".into())
        }
        Value::Float(f) => Ok(f.trunc() as i128),
        Value::Str(_) => {
            let t = py::str_of(v);
            let t = t.trim_matches(crate::families::py_space);
            let (neg, digits) = match t.strip_prefix('-') {
                Some(d) => (true, d),
                None => (false, t.strip_prefix('+').unwrap_or(t)),
            };
            let ok = !digits.is_empty()
                && !digits.starts_with('_')
                && !digits.ends_with('_')
                && !digits.contains("__")
                && digits.chars().all(|c| c.is_ascii_digit() || c == '_');
            if !ok {
                return Err(bad());
            }
            let n: i128 = digits.replace('_', "").parse().map_err(|_| bad())?;
            Ok(if neg { -n } else { n })
        }
        other => Err(format!(
            "TypeError: int() argument must be a string, a bytes-like object or a real number, not '{}'",
            py::type_name(other)
        )),
    }
}

/// A dict's key that must be hashable (`{...}.get(dr, dr)`).
pub fn hashable(v: &Value) -> R<()> {
    match v {
        Value::Array(_) | Value::Object(_) => Err(format!(
            "TypeError: unhashable type: '{}'",
            py::type_name(v)
        )),
        _ => Ok(()),
    }
}

/// `"%r" % v[:n]` of a string read from stdin.
pub fn repr_head(raw: &str, n: usize) -> String {
    py::repr(&s(&raw.chars().take(n).collect::<String>()))
}

/// `json.dumps(v, indent=2)`.
pub fn dumps_indent(v: &Value) -> R<String> {
    json::to_string(
        v,
        &json::Options {
            indent: Some(2),
            ..json::Options::default()
        },
    )
    .map_err(|e| format!("ValueError: {e:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_exceptions() {
        assert_eq!(msg("TypeError: a: b"), "a: b");
        assert_eq!(left(&s("ab"), 4).unwrap(), "ab  ");
        assert_eq!(left(&Value::Bool(true), 3).unwrap(), "1  ");
        assert_eq!(
            left(&Value::Null, 3).unwrap_err(),
            "TypeError: unsupported format string passed to NoneType.__format__"
        );
        assert_eq!(int(&s(" 1_0 ")).unwrap(), 10);
        assert_eq!(
            int(&s("x")).unwrap_err(),
            "ValueError: invalid literal for int() with base 10: 'x'"
        );
        assert_eq!(
            stamp(&s("2026-11-20T10:00:00.123Z")).unwrap(),
            "2026-11-20 10:00:00"
        );
        assert_eq!(join(&s("ab"), ",").unwrap(), "a,b");
    }
}
