//! The few Python operations `ci`'s readers use on a value `json.load`
//! gave, each raising what Python raises where the value is not what the
//! reader expects. A raise names the reader's first line: the rig compares
//! a traceback's exception, not its frames.

use crate::families::{py_d, py_iter, Raise};
use crate::out::json::{Text, Value};
use crate::out::py;
use crate::out::pylib::ops::{index, join};

pub type R<T> = Result<T, Raise>;

/// `v.get(k)`: `None` (Null) for a missing key.
pub fn g(v: &Value, k: &str, l: u32) -> R<Value> {
    Ok(crate::families::py_get(v, k, l)?
        .cloned()
        .unwrap_or(Value::Null))
}

/// `v.get(k, default)`.
pub fn g_or(v: &Value, k: &str, d: Value, l: u32) -> R<Value> {
    Ok(crate::families::py_get(v, k, l)?.cloned().unwrap_or(d))
}

/// `v[k]`.
pub fn ix(v: &Value, k: &str, l: u32) -> R<Value> {
    index(v, k, l).cloned()
}

/// `k in v`.
pub fn has(v: &Value, k: &str, l: u32) -> R<bool> {
    crate::out::pylib::ops::contains(v, k, l)
}

/// `a or b`.
pub fn or(a: Value, b: Value) -> Value {
    if py::truthy(&a) {
        a
    } else {
        b
    }
}

/// `str(v)`.
pub fn s(v: &Value) -> String {
    py::str_of(v)
}

/// A string value.
pub fn t(text: &str) -> Value {
    Value::Str(Text::from(text))
}

/// `v[:n]`.
pub fn head(v: &Value, n: usize, l: u32) -> R<Value> {
    match v {
        Value::Str(text) => {
            let cps = text.code_points();
            let cut: Vec<u32> = cps.into_iter().take(n).collect();
            Ok(Value::Str(from_points(&cut)))
        }
        Value::Array(items) => Ok(Value::Array(items.iter().take(n).cloned().collect())),
        Value::Object(_) => Err(Raise::at(l, "TypeError: unhashable type: 'slice'")),
        other => Err(Raise::at(
            l,
            format!(
                "TypeError: '{}' object is not subscriptable",
                py::type_name(other)
            ),
        )),
    }
}

fn from_points(cps: &[u32]) -> Text {
    let mut out = String::new();
    let mut lossy = false;
    for cp in cps {
        match char::from_u32(*cp) {
            Some(c) => out.push(c),
            None => lossy = true,
        }
    }
    if !lossy {
        return Text::from(out.as_str());
    }
    let mut joined = String::new();
    for cp in cps {
        joined.push_str(&py::str_of(&Value::Str(crate::families::code_point(*cp))));
    }
    Text::from(joined.as_str())
}

/// `", ".join(v or [])`.
pub fn joined(v: &Value, sep: &str, l: u32) -> R<String> {
    join(&or(v.clone(), Value::Array(Vec::new())), sep, l)
}

/// `for x in v`.
pub fn items(v: &Value, l: u32) -> R<Vec<Value>> {
    py_iter(v, l)
}

/// `"%d" % v`.
pub fn d(v: &Value, l: u32) -> R<String> {
    py_d(v, l)
}

/// `"%-<w>s" % v`.
pub fn left(v: &Value, w: usize) -> String {
    crate::families::py_left(v, w)
}

/// `int(v)`.
pub fn int(v: &Value, l: u32) -> R<i128> {
    match v {
        Value::Bool(b) => Ok(i128::from(*b)),
        Value::Int(digits) => digits
            .parse()
            .map_err(|_| Raise::at(l, "OverflowError: int too large")),
        Value::Float(f) if f.is_nan() => Err(Raise::at(
            l,
            "ValueError: cannot convert float NaN to integer",
        )),
        Value::Float(f) if f.is_infinite() => Err(Raise::at(
            l,
            "OverflowError: cannot convert float infinity to integer",
        )),
        Value::Float(f) => Ok(f.trunc() as i128),
        Value::Str(text) => {
            let raw = py::str_of(v);
            let st = raw.trim_matches(crate::families::py_space).replace('_', "");
            st.parse().map_err(|_| {
                Raise::at(
                    l,
                    format!(
                        "ValueError: invalid literal for int() with base 10: {}",
                        py::repr(&Value::Str(text.clone()))
                    ),
                )
            })
        }
        other => Err(Raise::at(
            l,
            format!(
                "TypeError: int() argument must be a string, a bytes-like object or a real number, not '{}'",
                py::type_name(other)
            ),
        )),
    }
}

/// The readers' `ago(t)`: `-`, `now`, or minutes, hours or days ago.
pub fn ago(v: &Value, now: i64, l: u32) -> R<String> {
    if !py::truthy(v) {
        return Ok("-".into());
    }
    let d = i128::from(now) - int(v, l)?;
    Ok(if d < 60 {
        "now".into()
    } else if d < 3600 {
        format!("{}m ago", d.div_euclid(60))
    } else if d < 86400 {
        format!("{}h ago", d.div_euclid(3600))
    } else {
        format!("{}d ago", d.div_euclid(86400))
    })
}

/// `time.strftime("%Y-%m-%d %H:%M UTC", time.gmtime(int(t)))`.
pub fn when(v: &Value, l: u32) -> R<String> {
    let secs = int(v, l)?;
    let secs = i64::try_from(secs).map_err(|_| {
        Raise::at(
            l,
            "OverflowError: timestamp out of range for platform time_t",
        )
    })?;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, dd) = crate::families::heartbeat::when::civil_from_days(days);
    Ok(format!(
        "{y:04}-{m:02}-{dd:02} {:02}:{:02} UTC",
        rem / 3600,
        (rem % 3600) / 60
    ))
}

/// The seconds since 1970, as `time.time()` reads them.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}
