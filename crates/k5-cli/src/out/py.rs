//! What Python prints for a value `json.loads` gave, where the shell CLI's
//! embedded Python formats one itself instead of through `json.dumps`:
//! `'%s' % v` ([`str_of`], [`repr`]), `'%d' % v` ([`percent_d`]) and
//! `if v:` ([`truthy`]).
//!
//! - `str()` of a string is the string; of anything else, `repr()`.
//! - `repr()` of a string picks its quote as Python does (`'`, or `"` when
//!   the text has a `'` and no `"`) and escapes the quote, `\`, `\t`, `\n`,
//!   `\r`, other control characters and what Python counts as not
//!   printable (`\xHH`, `\uHHHH`, `\UHHHHHHHH`). A list is `[a, b]`, an
//!   object `{'k': v}`, `null` `None`, `true` `True`, a float its shortest
//!   round trip (`1e+16`, `nan`, `inf`).
//!
//! Where this can differ from Python: "printable" outside Latin-1 is
//! Python's Unicode table; here it is every code point but the separator,
//! format, surrogate, private use and noncharacter ranges listed in
//! [`printable`], so a code point Unicode has not assigned yet is kept
//! where Python escapes it. A lone surrogate in a printed string is
//! written `\udXXX` (Python's `backslashreplace`), where Python's `print`
//! to a UTF-8 stdout fails. The daemon writes its JSON from Rust strings,
//! which hold neither.

use super::json::{self, Options, Text, Value};

/// Python's type name for the value (`type(v).__name__`).
pub fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Int(_) => "int",
        Value::Float(_) => "float",
        Value::Str(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// `bool(v)`.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Int(digits) => digits != "0",
        Value::Float(f) => *f != 0.0,
        Value::Str(t) => !t.code_points().is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(pairs) => !pairs.is_empty(),
    }
}

/// `str(v)`: what `'%s' % v` prints.
pub fn str_of(v: &Value) -> String {
    match v {
        Value::Str(t) => text(t),
        other => repr(other),
    }
}

/// A string as Python writes it out, a lone surrogate as `\udXXX`.
fn text(t: &Text) -> String {
    if let Some(s) = t.as_str() {
        return s.to_string();
    }
    t.code_points()
        .into_iter()
        .map(|cp| char::from_u32(cp).map_or_else(|| format!("\\u{cp:04x}"), String::from))
        .collect()
}

/// `repr(v)`.
pub fn repr(v: &Value) -> String {
    match v {
        Value::Null => "None".to_string(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Int(digits) => digits.clone(),
        Value::Float(f) => float_repr(*f),
        Value::Str(t) => str_repr(t),
        Value::Array(items) => format!("[{}]", items.iter().map(repr).collect::<Vec<_>>().join(", ")),
        Value::Object(pairs) => {
            let inner: Vec<String> = pairs.iter().map(|(k, v)| format!("{}: {}", str_repr(k), repr(v))).collect();
            format!("{{{}}}", inner.join(", "))
        }
    }
}

/// `repr(float)`: `json.dumps`'s text for a finite one (the same shortest
/// round trip), and Python's own words for the others.
fn float_repr(f: f64) -> String {
    if f.is_nan() {
        "nan".to_string()
    } else if f.is_infinite() {
        (if f > 0.0 { "inf" } else { "-inf" }).to_string()
    } else {
        json::to_string(&Value::Float(f), &Options::default()).expect("a finite float always prints")
    }
}

/// `repr(str)`.
fn str_repr(t: &Text) -> String {
    let points = t.code_points();
    let has = |c: char| points.contains(&u32::from(c));
    let quote = if has('\'') && !has('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(points.len() + 2);
    out.push(quote);
    for cp in points {
        match char::from_u32(cp) {
            Some(c) if c == quote || c == '\\' => {
                out.push('\\');
                out.push(c);
            }
            Some('\t') => out.push_str("\\t"),
            Some('\n') => out.push_str("\\n"),
            Some('\r') => out.push_str("\\r"),
            _ if cp < 0x20 || cp == 0x7f => out.push_str(&format!("\\x{cp:02x}")),
            Some(c) if cp < 0x7f => out.push(c),
            Some(c) if printable(cp) => out.push(c),
            _ if cp <= 0xff => out.push_str(&format!("\\x{cp:02x}")),
            _ if cp <= 0xffff => out.push_str(&format!("\\u{cp:04x}")),
            _ => out.push_str(&format!("\\U{cp:08x}")),
        }
    }
    out.push(quote);
    out
}

/// Python's `str.isprintable()` for one code point past ASCII: not a
/// separator (but the space), a format character, a surrogate, private
/// use or a noncharacter. Unassigned code points count as printable here.
fn printable(cp: u32) -> bool {
    const NOT: &[(u32, u32)] = &[
        (0x80, 0xa0),
        (0xad, 0xad),
        (0x600, 0x605),
        (0x61c, 0x61c),
        (0x6dd, 0x6dd),
        (0x70f, 0x70f),
        (0x890, 0x891),
        (0x8e2, 0x8e2),
        (0x1680, 0x1680),
        (0x180e, 0x180e),
        (0x2000, 0x200f),
        (0x2028, 0x202f),
        (0x205f, 0x2064),
        (0x2066, 0x206f),
        (0x3000, 0x3000),
        (0xd800, 0xf8ff),
        (0xfdd0, 0xfdef),
        (0xfeff, 0xfeff),
        (0xfff9, 0xfffb),
        (0x110bd, 0x110bd),
        (0x110cd, 0x110cd),
        (0x13430, 0x1343f),
        (0x1bca0, 0x1bca3),
        (0x1d173, 0x1d17a),
        (0xe0001, 0xe0001),
        (0xe0020, 0xe007f),
        (0xf0000, 0x10ffff),
    ];
    if cp & 0xfffe == 0xfffe {
        return false;
    }
    !NOT.iter().any(|(lo, hi)| (*lo..=*hi).contains(&cp))
}

/// `'%d' % v`: the digits, or the exception Python raises, as its last
/// traceback line (`TypeError: …`).
pub fn percent_d(v: &Value) -> Result<String, String> {
    match v {
        Value::Int(digits) => Ok(digits.clone()),
        Value::Bool(b) => Ok(if *b { "1" } else { "0" }.to_string()),
        Value::Float(f) if f.is_nan() => Err("ValueError: cannot convert float NaN to integer".to_string()),
        Value::Float(f) if f.is_infinite() => Err("OverflowError: cannot convert float infinity to integer".to_string()),
        Value::Float(f) => Ok(whole_digits(f.trunc())),
        other => Err(format!("TypeError: %d format: a real number is required, not {}", type_name(other))),
    }
}

/// The decimal digits of a whole, finite float, every one of them.
fn whole_digits(w: f64) -> String {
    if w.abs() < 9.0e15 {
        // Exact below 2^53.
        return format!("{}", w as i64);
    }
    // w = mantissa * 2^shift exactly, shift >= 1 here.
    let bits = w.abs().to_bits();
    let mantissa = (bits & ((1u64 << 52) - 1)) | (1u64 << 52);
    let shift = ((bits >> 52) & 0x7ff) as i32 - 1075;
    // Base 10^9 limbs, least significant first.
    let mut limbs: Vec<u64> = vec![mantissa % 1_000_000_000, (mantissa / 1_000_000_000) % 1_000_000_000, mantissa / 1_000_000_000_000_000_000];
    for _ in 0..shift {
        let mut carry = 0;
        for limb in limbs.iter_mut() {
            let x = *limb * 2 + carry;
            *limb = x % 1_000_000_000;
            carry = x / 1_000_000_000;
        }
        if carry > 0 {
            limbs.push(carry);
        }
    }
    while limbs.len() > 1 && limbs.last() == Some(&0) {
        limbs.pop();
    }
    let mut out = String::new();
    if w < 0.0 {
        out.push('-');
    }
    out.push_str(&limbs.last().expect("at least one limb").to_string());
    for limb in limbs.iter().rev().skip(1) {
        out.push_str(&format!("{limb:09}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Value {
        json::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"))
    }

    /// Each row is what Python 3 prints for `str(json.loads(text))`.
    #[test]
    fn str_and_repr_as_python_prints_them() {
        for (text, want) in [
            ("null", "None"),
            ("true", "True"),
            ("false", "False"),
            ("-0", "0"),
            ("12345678901234567890123", "12345678901234567890123"),
            ("1.5", "1.5"),
            ("1e16", "1e+16"),
            ("1E5", "100000.0"),
            ("NaN", "nan"),
            ("-Infinity", "-inf"),
            ("1e400", "inf"),
            (r#""plain é ☃""#, "plain é ☃"),
            ("[]", "[]"),
            ("{}", "{}"),
            (r#"[1, "a", null, [true]]"#, "[1, 'a', None, [True]]"),
            (r#"{"code": "x", "n": 1.0}"#, "{'code': 'x', 'n': 1.0}"),
            (r#"["it's"]"#, r#"["it's"]"#),
            (r#"["it's \"q\""]"#, r#"['it\'s "q"']"#),
            (r#"["a\\b\t\n\r\u0001\u007f"]"#, r"['a\\b\t\n\r\x01\x7f']"),
            (r#"["\u0085\u00a0\u00ad\u00e9\u2028\u200b\ufeff\ud83d\ude00"]"#, "['\\x85\\xa0\\xad\u{e9}\\u2028\\u200b\\ufeff\u{1f600}']"),
            (r#"["\ud83d", "\ue000", "\uffff", "\udb80\udc00"]"#, r"['\ud83d', '\ue000', '\uffff', '\U000f0000']"),
            (r#"{"k": {"x": []}}"#, "{'k': {'x': []}}"),
        ] {
            assert_eq!(str_of(&v(text)), want, "{text}");
        }
        assert_eq!(str_of(&v(r#""\ud83d!""#)), "\\ud83d!");
    }

    #[test]
    fn truth_as_python_sees_it() {
        for (text, want) in [
            ("null", false),
            ("false", false),
            ("true", true),
            ("0", false),
            ("-0", false),
            ("7", true),
            ("0.0", false),
            ("-0.0", false),
            ("NaN", true),
            (r#""""#, false),
            (r#"" ""#, true),
            ("[]", false),
            ("[0]", true),
            ("{}", false),
            (r#"{"a":0}"#, true),
        ] {
            assert_eq!(truthy(&v(text)), want, "{text}");
        }
    }

    #[test]
    fn percent_d_as_python_formats_it() {
        for (text, want) in [
            ("3", "3"),
            ("-12345678901234567890", "-12345678901234567890"),
            ("true", "1"),
            ("false", "0"),
            ("2.9", "2"),
            ("-2.9", "-2"),
            ("-0.0", "0"),
            ("1e15", "1000000000000000"),
            ("9007199254740993.0", "9007199254740992"),
            ("1e20", "100000000000000000000"),
            ("1.2345e25", "12344999999999999704301568"),
            ("-1e300", "-1000000000000000052504760255204420248704468581108159154915854115511802457988908195786371375080447864043704443832883878176942523235360430575644792184786706982848387200926575803737830233794788090059368953234970799945081119038967640880074652742780142494579258788820056842838115669472196386865459400540160"),
        ] {
            assert_eq!(percent_d(&v(text)).as_deref(), Ok(want), "{text}");
        }
        for (text, want) in [
            ("NaN", "ValueError: cannot convert float NaN to integer"),
            ("Infinity", "OverflowError: cannot convert float infinity to integer"),
            ("-Infinity", "OverflowError: cannot convert float infinity to integer"),
            (r#""3""#, "TypeError: %d format: a real number is required, not str"),
            ("null", "TypeError: %d format: a real number is required, not NoneType"),
            ("[]", "TypeError: %d format: a real number is required, not list"),
            ("{}", "TypeError: %d format: a real number is required, not dict"),
        ] {
            assert_eq!(percent_d(&v(text)).as_ref().map_err(String::as_str), Err(want), "{text}");
        }
    }
}
