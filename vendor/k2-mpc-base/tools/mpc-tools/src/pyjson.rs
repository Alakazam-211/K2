//! JSON written as Python's `json.dump` writes it (`ensure_ascii`, its `indent` layout, its
//! float text), so the files the ported tools write are byte for byte the ones they wrote
//! before. Key order is the map's own (serde_json keeps insertion order here).

use serde_json::Value;

/// Python's `repr` of a float, as `json.dumps` prints it (`NaN`, `Infinity` too).
pub fn float(f: f64) -> String {
    if f.is_nan() {
        return "NaN".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    if f == 0.0 {
        return if f.is_sign_negative() { "-0.0" } else { "0.0" }.to_string();
    }
    // the shortest digits that read back, and the decimal exponent of the first one
    let e = format!("{f:e}");
    let (mant, exp) = e.split_once('e').expect("a float in e notation");
    let exp: i32 = exp.parse().expect("an exponent");
    let neg = mant.starts_with('-');
    let digits: String = mant.chars().filter(char::is_ascii_digit).collect();
    let sign = if neg { "-" } else { "" };
    if (-4..16).contains(&exp) {
        let n = digits.len() as i32;
        let body = if exp < 0 {
            format!("0.{}{}", "0".repeat((-exp - 1) as usize), digits)
        } else if exp + 1 >= n {
            format!("{}{}.0", digits, "0".repeat((exp + 1 - n) as usize))
        } else {
            let (a, b) = digits.split_at((exp + 1) as usize);
            format!("{a}.{b}")
        };
        format!("{sign}{body}")
    } else {
        let (a, b) = digits.split_at(1);
        let m = if b.is_empty() {
            a.to_string()
        } else {
            format!("{a}.{b}")
        };
        let es = if exp < 0 { "-" } else { "+" };
        format!("{sign}{m}e{es}{:02}", exp.abs())
    }
}

/// A string as `json.dumps` writes it with `ensure_ascii` on.
pub fn string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            _ => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{u:04x}"));
                }
            }
        }
    }
    out.push('"');
}

fn number(n: &serde_json::Number) -> String {
    if n.is_i64() || n.is_u64() {
        n.to_string()
    } else {
        float(n.as_f64().unwrap_or(f64::NAN))
    }
}

/// `json.dumps(v, indent=indent)` (`None`: one line, `", "` and `": "` between items).
pub fn dumps(v: &Value, indent: Option<usize>) -> String {
    let mut out = String::new();
    write(v, indent, 0, &mut out);
    out
}

fn write(v: &Value, indent: Option<usize>, level: usize, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&number(n)),
        Value::String(s) => string(s, out),
        Value::Array(a) if a.is_empty() => out.push_str("[]"),
        Value::Object(o) if o.is_empty() => out.push_str("{}"),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                sep(i, indent, level + 1, out);
                write(x, indent, level + 1, out);
            }
            close(indent, level, out);
            out.push(']');
        }
        Value::Object(o) => {
            out.push('{');
            for (i, (k, x)) in o.iter().enumerate() {
                sep(i, indent, level + 1, out);
                string(k, out);
                out.push_str(": ");
                write(x, indent, level + 1, out);
            }
            close(indent, level, out);
            out.push('}');
        }
    }
}

fn sep(i: usize, indent: Option<usize>, level: usize, out: &mut String) {
    match indent {
        Some(w) => {
            if i > 0 {
                out.push(',');
            }
            out.push('\n');
            out.push_str(&" ".repeat(w * level));
        }
        None => {
            if i > 0 {
                out.push_str(", ");
            }
        }
    }
}

fn close(indent: Option<usize>, level: usize, out: &mut String) {
    if let Some(w) = indent {
        out.push('\n');
        out.push_str(&" ".repeat(w * level));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_as_python() {
        for (f, want) in [
            (1.0, "1.0"),
            (1.5, "1.5"),
            (0.1, "0.1"),
            (1e16, "1e+16"),
            (1.5e16, "1.5e+16"),
            (123456789012345.6, "123456789012345.6"),
            (1e-5, "1e-05"),
            (0.0001, "0.0001"),
            (-2.5, "-2.5"),
            (17116.4, "17116.4"),
            (100.0, "100.0"),
        ] {
            assert_eq!(float(f), want, "{f}");
        }
    }

    #[test]
    fn layout_as_python() {
        let v: Value = serde_json::from_str(r#"[{"a": 1, "b": [], "c": {}, "d": "é "}]"#).unwrap();
        assert_eq!(
            dumps(&v, Some(1)),
            "[\n {\n  \"a\": 1,\n  \"b\": [],\n  \"c\": {},\n  \"d\": \"\\u00e9\\u2028\"\n }\n]"
        );
        assert_eq!(
            dumps(&v, None),
            "[{\"a\": 1, \"b\": [], \"c\": {}, \"d\": \"\\u00e9\\u2028\"}]"
        );
    }
}
