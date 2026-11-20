//! The vault's Python printers' building blocks: `d.get(k)`, `x or {}`,
//! `"%s" % x` and the rest, each raising where Python raises, so a
//! printer can be written line for line from the shell's and end with
//! its traceback ([`render`]).

use crate::exit::Exit;
use crate::families::{py_get, py_iter, py_left, raised, Raise, Shown};
use crate::out::json::{self, Text, Value};
use crate::out::py;

/// A printer's step: a value, or where Python raised.
pub type R<T> = Result<T, Raise>;

/// `d.get(k)`: `None` (as `Value::Null`) for a missing key; the
/// `AttributeError` for anything but a dict.
pub fn get(d: &Value, k: &str) -> R<Value> {
    Ok(py_get(d, k, 1)?.cloned().unwrap_or(Value::Null))
}

/// `d.get(k, default)`.
pub fn get_or(d: &Value, k: &str, default: Value) -> R<Value> {
    Ok(py_get(d, k, 1)?.cloned().unwrap_or(default))
}

/// `d[k]`: the `KeyError` for a missing key, the `TypeError` for what
/// can't be indexed by a string.
pub fn idx(d: &Value, k: &str) -> R<Value> {
    match d {
        Value::Object(_) => match py_get(d, k, 1)? {
            Some(v) => Ok(v.clone()),
            None => Err(Raise::at(1, format!("KeyError: '{k}'"))),
        },
        Value::Array(_) => Err(Raise::at(
            1,
            "TypeError: list indices must be integers or slices, not str",
        )),
        Value::Str(_) => Err(Raise::at(
            1,
            "TypeError: string indices must be integers, not 'str'",
        )),
        other => Err(Raise::at(
            1,
            format!(
                "TypeError: '{}' object is not subscriptable",
                py::type_name(other)
            ),
        )),
    }
}

/// `x or {}`.
pub fn or_obj(v: Value) -> Value {
    if py::truthy(&v) {
        v
    } else {
        Value::Object(Vec::new())
    }
}

/// `x or []`.
pub fn or_list(v: Value) -> Value {
    if py::truthy(&v) {
        v
    } else {
        Value::Array(Vec::new())
    }
}

/// `for x in v`.
pub fn items(v: &Value) -> R<Vec<Value>> {
    py_iter(v, 1)
}

/// `str(v)`, as `"%s" % v` prints it.
pub fn s(v: &Value) -> String {
    py::str_of(v)
}

/// `bool(v)`.
pub fn t(v: &Value) -> bool {
    py::truthy(v)
}

/// `"%-<w>s" % v`.
pub fn left(v: &Value, w: usize) -> String {
    py_left(v, w)
}

/// A string value.
pub fn text(s: &str) -> Value {
    Value::Str(Text::from(s))
}

/// `a == "<s>"` for a value.
pub fn is(v: &Value, s: &str) -> bool {
    matches!(v, Value::Str(t) if t.as_str() == Some(s))
}

/// `"x" + v`: a string, or Python's `TypeError`.
pub fn cat(a: &str, v: &Value) -> R<String> {
    match v {
        Value::Str(_) => Ok(format!("{a}{}", s(v))),
        other => Err(Raise::at(
            1,
            format!(
                "TypeError: can only concatenate str (not \"{}\") to str",
                py::type_name(other)
            ),
        )),
    }
}

/// `"%d" % v`.
pub fn d(v: &Value) -> R<String> {
    py::percent_d(v).map_err(|e| Raise::at(1, e))
}

/// What a printer wrote, line by line.
#[derive(Default)]
pub struct Lines {
    pub out: String,
    pub err: String,
}

impl Lines {
    /// `print(x)`.
    pub fn p(&mut self, line: impl AsRef<str>) {
        self.out.push_str(line.as_ref());
        self.out.push('\n');
    }

    /// `print(x, file=sys.stderr)`.
    pub fn e(&mut self, line: impl AsRef<str>) {
        self.err.push_str(line.as_ref());
        self.err.push('\n');
    }
}

/// `printf '%s' "$_SC_BODY" | python3 -c '<printer>'`: the body as
/// `json.load` reads it, then the printer. `Ok(code)` is its
/// `sys.exit(code)` (0 when it ends); a raise prints what came before and
/// the traceback, exit 1.
pub fn render(body: &[u8], f: impl FnOnce(&Value, &mut Lines) -> R<i32>) -> Shown {
    let v = match crate::confirm::load(body) {
        Ok(v) => v,
        Err(r) => return raised(String::new(), String::new(), &r),
    };
    let mut l = Lines::default();
    match f(&v, &mut l) {
        Ok(code) => Shown {
            stdout: l.out,
            stderr: l.err,
            exit: if code == 0 { Exit::Ok } else { Exit::Error },
        },
        Err(r) => raised(l.out, l.err, &r),
    }
}

/// `urllib.parse.urlencode(o)` on a dict of strings: `quote_plus` on each
/// key and value (a space is `+`), joined by `&`.
pub fn urlencode(pairs: &[(Text, Value)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| {
            let k = json::Value::Str(k.clone());
            format!("{}={}", quote_plus(&s(&k)), quote_plus(&s(v)))
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// `urllib.parse.quote_plus(s, safe='')`.
pub fn quote_plus(s: &str) -> String {
    crate::transport::encode::quote(s).replace("%20", "+")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencode_is_python_s() {
        let o = vec![
            (Text::from("scope"), text("workspace")),
            (Text::from("workspace"), text("a b&c/é~")),
            (Text::from("name"), text("X_1.-")),
        ];
        assert_eq!(
            urlencode(&o),
            "scope=workspace&workspace=a+b%26c%2F%C3%A9~&name=X_1.-"
        );
    }

    #[test]
    fn gets_raise_where_python_raises() {
        let d = json::parse(r#"{"a":1}"#).unwrap();
        assert_eq!(get(&d, "b").unwrap(), Value::Null);
        assert!(get(&text("x"), "a").is_err());
        assert_eq!(idx(&d, "b").unwrap_err().exception, "KeyError: 'b'");
        assert_eq!(or_obj(Value::Null), Value::Object(Vec::new()));
        assert!(cat("x", &Value::Null).is_err());
    }
}
