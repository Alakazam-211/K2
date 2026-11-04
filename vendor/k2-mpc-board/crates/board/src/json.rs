//! Strict JSON: an RFC 8259 reader that refuses what would lose data, and an RFC 8785 canonical
//! writer. Our own, because a general parser keeps the last of two duplicate keys and takes a
//! number of any length, and both would be silent loss here.

use crate::limits::Limits;
use crate::refusal::{Refusal, RefusalCode};
use std::collections::BTreeMap;

/// A JSON object's members.
pub type Fields = BTreeMap<String, Value>;

/// A JSON value. Numbers are IEEE 754 doubles that passed the number rule in [`number_value`].
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A number.
    Num(f64),
    /// A string.
    Str(String),
    /// An array, in its order.
    Arr(Vec<Value>),
    /// An object.
    Obj(Fields),
}

/// The largest magnitude a number may have: 2^53.
pub const MAX_NUMBER: f64 = 9_007_199_254_740_992.0;

/// The most significant digits a number literal may have: enough to print any double exactly.
pub const MAX_SIGNIFICANT_DIGITS: usize = 17;

/// The longest number literal, in bytes.
pub const MAX_NUMBER_LITERAL: usize = 64;

/// Parses one JSON document under these limits.
pub fn parse(bytes: &[u8], limits: &Limits) -> Result<Value, Refusal> {
    let text = std::str::from_utf8(bytes).map_err(|e| {
        Refusal::at(
            RefusalCode::JsonUtf8,
            format!("byte {}", e.valid_up_to()),
            "not valid UTF-8",
        )
    })?;
    let mut p = Parser {
        text,
        b: text.as_bytes(),
        i: 0,
        depth: 0,
        values: 0,
        limits,
    };
    p.ws();
    let v = p.value()?;
    p.ws();
    if p.i != p.b.len() {
        return Err(p.syntax("content after the document"));
    }
    Ok(v)
}

struct Parser<'a> {
    text: &'a str,
    b: &'a [u8],
    i: usize,
    depth: u64,
    values: u64,
    limits: &'a Limits,
}

impl Parser<'_> {
    fn syntax(&self, what: &str) -> Refusal {
        Refusal::at(RefusalCode::JsonSyntax, format!("byte {}", self.i), what)
    }

    fn ws(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.b.get(self.i) {
            self.i += 1;
        }
    }

    fn enter(&mut self) -> Result<(), Refusal> {
        self.depth += 1;
        if self.depth > self.limits.depth {
            return Err(Refusal::at(
                RefusalCode::LimitDepth,
                format!("byte {}", self.i),
                format!("nested deeper than {}", self.limits.depth),
            ));
        }
        Ok(())
    }

    fn value(&mut self) -> Result<Value, Refusal> {
        self.values += 1;
        if self.values > self.limits.values {
            return Err(Refusal::at(
                RefusalCode::LimitValues,
                format!("byte {}", self.i),
                format!("more than {} values", self.limits.values),
            ));
        }
        match self.b.get(self.i) {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(Value::Str(self.string()?)),
            Some(b't') => self.word("true", Value::Bool(true)),
            Some(b'f') => self.word("false", Value::Bool(false)),
            Some(b'n') => self.word("null", Value::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(self.syntax("expected a value")),
        }
    }

    fn word(&mut self, w: &str, v: Value) -> Result<Value, Refusal> {
        if self.text[self.i..].starts_with(w) {
            self.i += w.len();
            Ok(v)
        } else {
            Err(self.syntax("expected a value"))
        }
    }

    fn object(&mut self) -> Result<Value, Refusal> {
        self.enter()?;
        self.i += 1;
        let mut out = Fields::new();
        self.ws();
        if self.b.get(self.i) == Some(&b'}') {
            self.i += 1;
            self.depth -= 1;
            return Ok(Value::Obj(out));
        }
        loop {
            self.ws();
            if self.b.get(self.i) != Some(&b'"') {
                return Err(self.syntax("expected a member name"));
            }
            let at = self.i;
            let key = self.string()?;
            self.ws();
            if self.b.get(self.i) != Some(&b':') {
                return Err(self.syntax("expected a colon"));
            }
            self.i += 1;
            self.ws();
            let v = self.value()?;
            if out.contains_key(&key) {
                return Err(Refusal::at(
                    RefusalCode::JsonDuplicateKey,
                    format!("byte {at}"),
                    format!("duplicate member {key:?}"),
                ));
            }
            out.insert(key, v);
            self.ws();
            match self.b.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    self.depth -= 1;
                    return Ok(Value::Obj(out));
                }
                _ => return Err(self.syntax("expected a comma or closing brace")),
            }
        }
    }

    fn array(&mut self) -> Result<Value, Refusal> {
        self.enter()?;
        self.i += 1;
        let mut out = Vec::new();
        self.ws();
        if self.b.get(self.i) == Some(&b']') {
            self.i += 1;
            self.depth -= 1;
            return Ok(Value::Arr(out));
        }
        loop {
            self.ws();
            out.push(self.value()?);
            self.ws();
            match self.b.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    self.depth -= 1;
                    return Ok(Value::Arr(out));
                }
                _ => return Err(self.syntax("expected a comma or closing bracket")),
            }
        }
    }

    fn string(&mut self) -> Result<String, Refusal> {
        self.i += 1;
        let mut out = String::new();
        loop {
            let start = self.i;
            while let Some(&c) = self.b.get(self.i) {
                if c == b'"' || c == b'\\' || c < 0x20 {
                    break;
                }
                self.i += 1;
            }
            // Stops only on ASCII bytes, so both ends are character boundaries.
            out.push_str(&self.text[start..self.i]);
            match self.b.get(self.i) {
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.i += 1;
                    self.escape(&mut out)?;
                }
                Some(_) => return Err(self.syntax("control character in a string")),
                None => return Err(self.syntax("unterminated string")),
            }
        }
    }

    fn escape(&mut self, out: &mut String) -> Result<(), Refusal> {
        let c = *self
            .b
            .get(self.i)
            .ok_or_else(|| self.syntax("unterminated string"))?;
        self.i += 1;
        match c {
            b'"' => out.push('"'),
            b'\\' => out.push('\\'),
            b'/' => out.push('/'),
            b'b' => out.push('\u{8}'),
            b'f' => out.push('\u{c}'),
            b'n' => out.push('\n'),
            b'r' => out.push('\r'),
            b't' => out.push('\t'),
            b'u' => {
                let u = self.hex4()?;
                let code = match u {
                    0xD800..=0xDBFF => {
                        if self.b.get(self.i) != Some(&b'\\')
                            || self.b.get(self.i + 1) != Some(&b'u')
                        {
                            return Err(self.surrogate());
                        }
                        self.i += 2;
                        let low = self.hex4()?;
                        if !(0xDC00..=0xDFFF).contains(&low) {
                            return Err(self.surrogate());
                        }
                        0x10000 + ((u - 0xD800) << 10) + (low - 0xDC00)
                    }
                    0xDC00..=0xDFFF => return Err(self.surrogate()),
                    _ => u,
                };
                // Surrogates are ruled out above, so this is always a character.
                out.push(char::from_u32(code).ok_or_else(|| self.surrogate())?);
            }
            _ => return Err(self.syntax("unknown escape")),
        }
        Ok(())
    }

    fn surrogate(&self) -> Refusal {
        Refusal::at(
            RefusalCode::JsonSurrogate,
            format!("byte {}", self.i),
            "a lone surrogate is not text",
        )
    }

    fn hex4(&mut self) -> Result<u32, Refusal> {
        let mut v = 0u32;
        for _ in 0..4 {
            let d = match self.b.get(self.i) {
                Some(c @ b'0'..=b'9') => c - b'0',
                Some(c @ b'a'..=b'f') => c - b'a' + 10,
                Some(c @ b'A'..=b'F') => c - b'A' + 10,
                _ => return Err(self.syntax("expected four hex digits")),
            };
            v = v * 16 + u32::from(d);
            self.i += 1;
        }
        Ok(v)
    }

    fn number(&mut self) -> Result<Value, Refusal> {
        let start = self.i;
        if self.b.get(self.i) == Some(&b'-') {
            self.i += 1;
        }
        match self.b.get(self.i) {
            Some(b'0') => self.i += 1,
            Some(b'1'..=b'9') => self.digits(),
            _ => return Err(self.syntax("bad number")),
        }
        if self.b.get(self.i) == Some(&b'.') {
            self.i += 1;
            if !matches!(self.b.get(self.i), Some(b'0'..=b'9')) {
                return Err(self.syntax("bad number"));
            }
            self.digits();
        }
        if let Some(b'e' | b'E') = self.b.get(self.i) {
            self.i += 1;
            if let Some(b'+' | b'-') = self.b.get(self.i) {
                self.i += 1;
            }
            if !matches!(self.b.get(self.i), Some(b'0'..=b'9')) {
                return Err(self.syntax("bad number"));
            }
            self.digits();
        }
        // A digit straight after `0`, as in `01`, is not part of any JSON number.
        if matches!(self.b.get(self.i), Some(b'0'..=b'9')) {
            return Err(self.syntax("bad number"));
        }
        let lit = &self.text[start..self.i];
        if lit.len() > MAX_NUMBER_LITERAL {
            return Err(Refusal::at(
                RefusalCode::LimitNumberLiteral,
                format!("byte {start}"),
                format!("a number literal over {MAX_NUMBER_LITERAL} bytes"),
            ));
        }
        number_value(lit).map(Value::Num).map_err(|mut r| {
            r.at = Some(format!("byte {start}"));
            r
        })
    }

    fn digits(&mut self) {
        while let Some(b'0'..=b'9') = self.b.get(self.i) {
            self.i += 1;
        }
    }
}

/// The number rule, for a literal already known to fit JSON's grammar.
///
/// Significant digits run from the first non-zero digit to the last, so `1.0000` has one. A literal
/// is accepted if its exact value is at most 2^53 in magnitude and it has at most 17 significant
/// digits, the most a writer needs to print any double. It lands on the nearest double, and a writer
/// prints that in its shortest form. So `0.10000000000000001` becomes `0.1`, `1.0` becomes `1`, and
/// `-0` becomes `0`.
///
/// Refused: `json_number_range` for a magnitude past 2^53, judged on the literal because
/// `9007199254740993` parses to 2^53; `json_number_inexact` for more digits than that, as in
/// `0.1000000000000000055`, and for a non-zero literal that would become zero, as in `1e-400`.
pub fn number_value(lit: &str) -> Result<f64, Refusal> {
    let (digits, exp) = decimal(lit);
    if digits.is_empty() {
        return Ok(0.0);
    }
    if past_max(&digits, exp) {
        return Err(Refusal::new(
            RefusalCode::JsonNumberRange,
            format!("{lit} is past 2^53"),
        ));
    }
    let inexact = |why: &str| Refusal::new(RefusalCode::JsonNumberInexact, format!("{lit} {why}"));
    if digits.len() > MAX_SIGNIFICANT_DIGITS {
        return Err(inexact("has more digits than a double holds"));
    }
    let v: f64 = lit
        .parse()
        .map_err(|_| Refusal::new(RefusalCode::JsonSyntax, format!("{lit} is not a number")))?;
    if v == 0.0 {
        return Err(inexact("is too small for a double, and would become zero"));
    }
    Ok(v)
}

/// A decimal literal as its significant digits and an exponent, with value 0.DIGITS x 10^exp. No
/// digits means zero. An exponent too long for an `i64` is held at a size no literal can undo.
fn decimal(lit: &str) -> (String, i64) {
    const HUGE: i64 = i64::MAX / 2;
    let rest = lit.strip_prefix('-').unwrap_or(lit);
    let (mantissa, exp) = match rest.find(['e', 'E']) {
        Some(i) => {
            let text = &rest[i + 1..];
            let held = if text.starts_with('-') { -HUGE } else { HUGE };
            (
                &rest[..i],
                text.parse::<i64>().unwrap_or(held).clamp(-HUGE, HUGE),
            )
        }
        None => (rest, 0),
    };
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let joined = format!("{int}{frac}");
    let kept = joined.trim_start_matches('0');
    let leading = (joined.len() - kept.len()) as i64;
    let digits = kept.trim_end_matches('0').to_string();
    (digits, exp + int.len() as i64 - leading)
}

/// Whether the exact value 0.DIGITS x 10^exp is past 2^53, with no rounding.
fn past_max(digits: &str, exp: i64) -> bool {
    const MAX_DIGITS: &str = "9007199254740992";
    match exp.cmp(&(MAX_DIGITS.len() as i64)) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        // Same number of integer digits, so text order is number order. Digits that start with all
        // of 2^53's and go on are past it, and digits that stop early aren't.
        std::cmp::Ordering::Equal => digits > MAX_DIGITS,
    }
}

/// The RFC 8785 form of a number: ECMAScript's `Number::toString`.
pub fn number_to_string(v: f64) -> Result<String, Refusal> {
    if !v.is_finite() || v.abs() > MAX_NUMBER {
        return Err(Refusal::new(
            RefusalCode::JsonNumberRange,
            format!("{v} is not a finite number up to 2^53"),
        ));
    }
    if v == 0.0 {
        return Ok("0".to_string());
    }
    let printed = format!("{:e}", v.abs());
    let (mantissa, exp) = printed.split_once('e').unwrap_or((&printed, "0"));
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i64;
    let n = exp.parse::<i64>().unwrap_or(0) + 1;
    let mut out = String::new();
    if v < 0.0 {
        out.push('-');
    }
    if k <= n && n <= 21 {
        out.push_str(&digits);
        out.push_str(&"0".repeat((n - k) as usize));
    } else if 0 < n && n <= 21 {
        out.push_str(&digits[..n as usize]);
        out.push('.');
        out.push_str(&digits[n as usize..]);
    } else if -6 < n && n <= 0 {
        out.push_str("0.");
        out.push_str(&"0".repeat((-n) as usize));
        out.push_str(&digits);
    } else {
        out.push_str(&digits[..1]);
        if k > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if n - 1 < 0 { '-' } else { '+' });
        out.push_str(&(n - 1).abs().to_string());
    }
    Ok(out)
}

/// The canonical bytes of a value: RFC 8785. Members sort by UTF-16 code units, numbers print in
/// their shortest form, strings escape only what JSON requires.
pub fn to_canonical(v: &Value) -> Result<Vec<u8>, Refusal> {
    let mut out = Vec::new();
    write_value(v, &mut out, 1)?;
    Ok(out)
}

fn write_value(v: &Value, out: &mut Vec<u8>, depth: u64) -> Result<(), Refusal> {
    match v {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Num(n) => out.extend_from_slice(number_to_string(*n)?.as_bytes()),
        Value::Str(s) => write_string(s, out),
        Value::Arr(items) => {
            check_depth(depth)?;
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_value(item, out, depth + 1)?;
            }
            out.push(b']');
        }
        Value::Obj(fields) => {
            check_depth(depth)?;
            let mut keys: Vec<&String> = fields.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            out.push(b'{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_string(k, out);
                out.push(b':');
                write_value(&fields[*k], out, depth + 1)?;
            }
            out.push(b'}');
        }
    }
    Ok(())
}

fn check_depth(depth: u64) -> Result<(), Refusal> {
    if depth > Limits::V1.depth {
        return Err(Refusal::new(
            RefusalCode::LimitDepth,
            format!("nested deeper than {}", Limits::V1.depth),
        ));
    }
    Ok(())
}

fn write_string(s: &str, out: &mut Vec<u8>) {
    out.push(b'"');
    let mut start = 0;
    let bytes = s.as_bytes();
    for (i, &c) in bytes.iter().enumerate() {
        let esc: Option<&[u8]> = match c {
            b'"' => Some(b"\\\""),
            b'\\' => Some(b"\\\\"),
            0x08 => Some(b"\\b"),
            0x0c => Some(b"\\f"),
            b'\n' => Some(b"\\n"),
            b'\r' => Some(b"\\r"),
            b'\t' => Some(b"\\t"),
            _ => None,
        };
        if let Some(e) = esc {
            out.extend_from_slice(&bytes[start..i]);
            out.extend_from_slice(e);
            start = i + 1;
        } else if c < 0x20 {
            out.extend_from_slice(&bytes[start..i]);
            out.extend_from_slice(format!("\\u{c:04x}").as_bytes());
            start = i + 1;
        }
    }
    out.extend_from_slice(&bytes[start..]);
    out.push(b'"');
}
