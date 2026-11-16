//! A small, bounded JSON reader of our own, for OTIO files and the JSON some formats embed.
//!
//! It reads RFC 8259 JSON, plus what OTIO's own writer emits for non-finite numbers: `NaN`,
//! `Infinity`, `-Infinity`, `Inf` and `-Inf` as number tokens. Numbers stay as their text, so a
//! value is read exactly ([`Num::ratio`]) and a huge integer is not rounded. It refuses: input
//! that is not UTF-8, a raw control character in a string, a lone surrogate escape, a key
//! repeated in one object, nesting deeper than its limit, more values than its limit, and
//! anything after the top-level value.

use crate::error::ReadError;

/// A JSON value. Objects keep their keys in file order.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Json {
    Null,
    Bool(bool),
    Num(Num),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

/// A number token, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Num(String);

impl Num {
    /// The value as an `f64` (non-finite tokens as infinities and NaN).
    pub(crate) fn as_f64(&self) -> f64 {
        match self.0.as_str() {
            "NaN" => f64::NAN,
            "Infinity" | "Inf" => f64::INFINITY,
            "-Infinity" | "-Inf" => f64::NEG_INFINITY,
            t => t.parse().unwrap_or(f64::NAN),
        }
    }

    /// The value exactly, as `numerator / denominator` (denominator above zero), when it is
    /// finite and fits: an integer, a decimal, or either with an exponent of at most 18.
    pub(crate) fn ratio(&self) -> Option<(i128, i128)> {
        let t = self.0.as_str();
        let (mant, exp) = match t.find(['e', 'E']) {
            Some(i) => (&t[..i], t[i + 1..].parse::<i32>().ok()?),
            None => (t, 0),
        };
        if !(-18..=18).contains(&exp) {
            return None;
        }
        let (neg, mant) = match mant.strip_prefix('-') {
            Some(m) => (true, m),
            None => (false, mant),
        };
        let (whole, frac) = mant.split_once('.').unwrap_or((mant, ""));
        if whole.is_empty() || frac.len() > 18 {
            return None;
        }
        let mut n: i128 = 0;
        for b in whole.bytes().chain(frac.bytes()) {
            if !b.is_ascii_digit() {
                return None;
            }
            n = n.checked_mul(10)?.checked_add(i128::from(b - b'0'))?;
        }
        let mut d: i128 = 10i128.pow(frac.len() as u32);
        let scale = 10i128.pow(exp.unsigned_abs());
        if exp >= 0 {
            n = n.checked_mul(scale)?;
        } else {
            d = d.checked_mul(scale)?;
        }
        Some((if neg { -n } else { n }, d))
    }

    /// The value as an `i64`, when it is an integer that fits.
    pub(crate) fn as_i64(&self) -> Option<i64> {
        let (n, d) = self.ratio()?;
        if n % d != 0 {
            return None;
        }
        i64::try_from(n / d).ok()
    }
}

impl Json {
    /// The value under `key`, when this is an object holding it.
    pub(crate) fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(kv) => kv.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// This value as a string.
    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    /// This value as an array.
    pub(crate) fn as_arr(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }

    /// This value as a boolean.
    pub(crate) fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Whether this is null.
    pub(crate) fn is_null(&self) -> bool {
        matches!(self, Json::Null)
    }

    /// Whether this is an object with no entries, or null.
    pub(crate) fn is_empty_obj(&self) -> bool {
        match self {
            Json::Obj(kv) => kv.is_empty(),
            Json::Null => true,
            _ => false,
        }
    }

    /// A canonical encoding, for digests: compact, object keys sorted, numbers as written.
    pub(crate) fn canonical(&self, out: &mut Vec<u8>) {
        match self {
            Json::Null => out.extend_from_slice(b"null"),
            Json::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
            Json::Num(n) => out.extend_from_slice(n.0.as_bytes()),
            Json::Str(s) => write_str(s, out),
            Json::Arr(a) => {
                out.push(b'[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    v.canonical(out);
                }
                out.push(b']');
            }
            Json::Obj(kv) => {
                let mut sorted: Vec<&(String, Json)> = kv.iter().collect();
                sorted.sort_by(|a, b| a.0.cmp(&b.0));
                out.push(b'{');
                for (i, (k, v)) in sorted.into_iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    write_str(k, out);
                    out.push(b':');
                    v.canonical(out);
                }
                out.push(b'}');
            }
        }
    }
}

fn write_str(s: &str, out: &mut Vec<u8>) {
    out.push(b'"');
    for c in s.chars() {
        match c {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            c if (c as u32) < 0x20 => {
                out.extend_from_slice(format!("\\u{:04x}", c as u32).as_bytes())
            }
            c => {
                let mut b = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
            }
        }
    }
    out.push(b'"');
}

/// Reads `bytes` as one JSON value, under `max_depth` nesting levels and `max_values` values.
pub(crate) fn parse(bytes: &[u8], max_depth: u32, max_values: u64) -> Result<Json, ReadError> {
    let mut p = Parser::over(bytes, max_depth, max_values)?;
    p.ws();
    let v = p.value(0)?;
    p.ws();
    if p.at != p.s.len() {
        return Err(p.err("text after the value"));
    }
    Ok(v)
}

/// A JSON value with the byte range it was read from, for a writer that splices: every value,
/// and every object key, knows where it is in the file. Offsets count from the file's first
/// byte (a byte order mark included), so `&bytes[node.start..node.end]` is the value as written.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// The value's first byte.
    pub start: usize,
    /// One past its last byte.
    pub end: usize,
    /// What it is.
    pub v: NodeValue,
}

/// A [`Node`]'s value. Strings are unescaped; numbers are their text as written.
#[derive(Debug, Clone, PartialEq)]
pub enum NodeValue {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A number, as written.
    Num(Num),
    /// A string, unescaped.
    Str(String),
    /// An array.
    Arr(Vec<Node>),
    /// Entries in file order: the key, where the key's opening quote is, and the value.
    Obj(Vec<(String, usize, Node)>),
}

impl Num {
    /// The number's text as written.
    pub(crate) fn text(&self) -> &str {
        &self.0
    }
}

impl Node {
    /// The value under `key`, when this is an object holding it.
    pub(crate) fn get(&self, key: &str) -> Option<&Node> {
        match &self.v {
            NodeValue::Obj(kv) => kv.iter().find(|(k, _, _)| k == key).map(|(_, _, v)| v),
            _ => None,
        }
    }

    /// The entries, when this is an object.
    pub(crate) fn entries(&self) -> Option<&[(String, usize, Node)]> {
        match &self.v {
            NodeValue::Obj(kv) => Some(kv),
            _ => None,
        }
    }

    /// This value as a string.
    pub(crate) fn as_str(&self) -> Option<&str> {
        match &self.v {
            NodeValue::Str(s) => Some(s),
            _ => None,
        }
    }

    /// This value as an array.
    pub(crate) fn as_arr(&self) -> Option<&[Node]> {
        match &self.v {
            NodeValue::Arr(a) => Some(a),
            _ => None,
        }
    }

    /// This value as a number.
    pub(crate) fn as_num(&self) -> Option<&Num> {
        match &self.v {
            NodeValue::Num(n) => Some(n),
            _ => None,
        }
    }

    /// This value as a boolean.
    pub(crate) fn as_bool(&self) -> Option<bool> {
        match &self.v {
            NodeValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Whether this is null.
    pub(crate) fn is_null(&self) -> bool {
        matches!(self.v, NodeValue::Null)
    }

    /// The same value without its ranges.
    pub(crate) fn to_json(&self) -> Json {
        match &self.v {
            NodeValue::Null => Json::Null,
            NodeValue::Bool(b) => Json::Bool(*b),
            NodeValue::Num(n) => Json::Num(n.clone()),
            NodeValue::Str(s) => Json::Str(s.clone()),
            NodeValue::Arr(a) => Json::Arr(a.iter().map(Node::to_json).collect()),
            NodeValue::Obj(kv) => Json::Obj(
                kv.iter()
                    .map(|(k, _, v)| (k.clone(), v.to_json()))
                    .collect(),
            ),
        }
    }
}

/// `s` as a JSON string literal, quotes included: what a splice writes for a new string.
pub(crate) fn quote_str(s: &str) -> String {
    let mut out = Vec::new();
    write_str(s, &mut out);
    // write_str writes UTF-8 and ASCII escapes only
    String::from_utf8(out).unwrap_or_default()
}

/// Reads `bytes` as one JSON value with every value's byte range ([`Node`]), under the same
/// limits and refusals as [`parse`].
pub(crate) fn parse_spanned(
    bytes: &[u8],
    max_depth: u32,
    max_values: u64,
) -> Result<Node, ReadError> {
    let mut p = Parser::over(bytes, max_depth, max_values)?;
    p.ws();
    let v = p.node(0)?;
    p.ws();
    if p.at != p.s.len() {
        return Err(p.err("text after the value"));
    }
    Ok(v)
}

struct Parser<'a> {
    s: &'a [u8],
    at: usize,
    max_depth: u32,
    max_values: u64,
    values: u64,
}

impl<'a> Parser<'a> {
    /// A parser over `bytes`, which must be UTF-8, past a byte order mark.
    fn over(bytes: &'a [u8], max_depth: u32, max_values: u64) -> Result<Self, ReadError> {
        std::str::from_utf8(bytes).map_err(|e| ReadError::Syntax {
            format: "utf-8",
            at: e.valid_up_to() as u64,
            why: "not UTF-8",
        })?;
        Ok(Parser {
            s: bytes,
            at: if bytes.starts_with(b"\xef\xbb\xbf") {
                3
            } else {
                0
            },
            max_depth,
            max_values,
            values: 0,
        })
    }

    /// One value with its range ([`Node`]).
    fn node(&mut self, depth: u32) -> Result<Node, ReadError> {
        let start = self.at;
        let v = match self.peek() {
            Some(b'{') => {
                self.values += 1;
                if self.values > self.max_values {
                    return Err(ReadError::Limit {
                        what: "JSON values",
                        limit: self.max_values,
                    });
                }
                self.deeper(depth)?;
                self.at += 1;
                self.node_object(depth + 1)?
            }
            Some(b'[') => {
                self.values += 1;
                if self.values > self.max_values {
                    return Err(ReadError::Limit {
                        what: "JSON values",
                        limit: self.max_values,
                    });
                }
                self.deeper(depth)?;
                self.at += 1;
                self.node_array(depth + 1)?
            }
            _ => match self.value(depth)? {
                Json::Null => NodeValue::Null,
                Json::Bool(b) => NodeValue::Bool(b),
                Json::Num(n) => NodeValue::Num(n),
                Json::Str(s) => NodeValue::Str(s),
                // `value` reads arrays and objects only after `[` and `{`, handled above
                Json::Arr(_) | Json::Obj(_) => return Err(self.err("not a value")),
            },
        };
        Ok(Node {
            start,
            end: self.at,
            v,
        })
    }

    fn node_object(&mut self, depth: u32) -> Result<NodeValue, ReadError> {
        let mut kv: Vec<(String, usize, Node)> = Vec::new();
        self.ws();
        if self.peek() == Some(b'}') {
            self.at += 1;
            return Ok(NodeValue::Obj(kv));
        }
        loop {
            self.ws();
            if self.peek() != Some(b'"') {
                return Err(self.err("an object key is a string"));
            }
            let key_at = self.at;
            let k = self.string()?;
            self.ws();
            if self.peek() != Some(b':') {
                return Err(self.err("expected : after a key"));
            }
            self.at += 1;
            self.ws();
            let v = self.node(depth)?;
            if kv.iter().any(|(e, _, _)| *e == k) {
                return Err(self.err("a key repeats in one object"));
            }
            kv.push((k, key_at, v));
            self.ws();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b'}') => {
                    self.at += 1;
                    return Ok(NodeValue::Obj(kv));
                }
                _ => return Err(self.err("expected , or } in an object")),
            }
        }
    }

    fn node_array(&mut self, depth: u32) -> Result<NodeValue, ReadError> {
        let mut a = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.at += 1;
            return Ok(NodeValue::Arr(a));
        }
        loop {
            self.ws();
            a.push(self.node(depth)?);
            self.ws();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b']') => {
                    self.at += 1;
                    return Ok(NodeValue::Arr(a));
                }
                _ => return Err(self.err("expected , or ] in an array")),
            }
        }
    }

    fn err(&self, why: &'static str) -> ReadError {
        ReadError::Syntax {
            format: "json",
            at: self.at as u64,
            why,
        }
    }

    fn ws(&mut self) {
        while let Some(&b) = self.s.get(self.at) {
            if matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
                self.at += 1;
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.at).copied()
    }

    fn eat(&mut self, lit: &[u8]) -> bool {
        if self.s[self.at..].starts_with(lit) {
            self.at += lit.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: u32) -> Result<Json, ReadError> {
        self.values += 1;
        if self.values > self.max_values {
            return Err(ReadError::Limit {
                what: "JSON values",
                limit: self.max_values,
            });
        }
        match self.peek() {
            None => Err(self.err("the text ends before a value")),
            Some(b'{') => {
                self.deeper(depth)?;
                self.at += 1;
                self.object(depth + 1)
            }
            Some(b'[') => {
                self.deeper(depth)?;
                self.at += 1;
                self.array(depth + 1)
            }
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') if self.eat(b"true") => Ok(Json::Bool(true)),
            Some(b'f') if self.eat(b"false") => Ok(Json::Bool(false)),
            Some(b'n') if self.eat(b"null") => Ok(Json::Null),
            Some(b'N') if self.eat(b"NaN") => Ok(Json::Num(Num("NaN".into()))),
            Some(b'I') if self.eat(b"Infinity") => Ok(Json::Num(Num("Infinity".into()))),
            Some(b'I') if self.eat(b"Inf") => Ok(Json::Num(Num("Inf".into()))),
            Some(b'-') if self.eat(b"-Infinity") => Ok(Json::Num(Num("-Infinity".into()))),
            Some(b'-') if self.eat(b"-Inf") => Ok(Json::Num(Num("-Inf".into()))),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.err("not a value")),
        }
    }

    fn deeper(&self, depth: u32) -> Result<(), ReadError> {
        if depth >= self.max_depth {
            return Err(ReadError::Limit {
                what: "nesting levels",
                limit: u64::from(self.max_depth),
            });
        }
        Ok(())
    }

    fn object(&mut self, depth: u32) -> Result<Json, ReadError> {
        let mut kv: Vec<(String, Json)> = Vec::new();
        self.ws();
        if self.peek() == Some(b'}') {
            self.at += 1;
            return Ok(Json::Obj(kv));
        }
        loop {
            self.ws();
            if self.peek() != Some(b'"') {
                return Err(self.err("an object key is a string"));
            }
            let k = self.string()?;
            self.ws();
            if self.peek() != Some(b':') {
                return Err(self.err("expected : after a key"));
            }
            self.at += 1;
            self.ws();
            let v = self.value(depth)?;
            if kv.iter().any(|(e, _)| *e == k) {
                return Err(self.err("a key repeats in one object"));
            }
            kv.push((k, v));
            self.ws();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b'}') => {
                    self.at += 1;
                    return Ok(Json::Obj(kv));
                }
                _ => return Err(self.err("expected , or } in an object")),
            }
        }
    }

    fn array(&mut self, depth: u32) -> Result<Json, ReadError> {
        let mut a = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.at += 1;
            return Ok(Json::Arr(a));
        }
        loop {
            self.ws();
            a.push(self.value(depth)?);
            self.ws();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b']') => {
                    self.at += 1;
                    return Ok(Json::Arr(a));
                }
                _ => return Err(self.err("expected , or ] in an array")),
            }
        }
    }

    fn number(&mut self) -> Result<Json, ReadError> {
        let start = self.at;
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        match self.peek() {
            Some(b'0') => self.at += 1,
            Some(b'1'..=b'9') => {
                while self.peek().is_some_and(|b| b.is_ascii_digit()) {
                    self.at += 1;
                }
            }
            _ => return Err(self.err("a number needs digits")),
        }
        if self.peek() == Some(b'.') {
            self.at += 1;
            if !self.peek().is_some_and(|b| b.is_ascii_digit()) {
                return Err(self.err("a number needs digits after the point"));
            }
            while self.peek().is_some_and(|b| b.is_ascii_digit()) {
                self.at += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.at += 1;
            }
            if !self.peek().is_some_and(|b| b.is_ascii_digit()) {
                return Err(self.err("a number needs digits in its exponent"));
            }
            while self.peek().is_some_and(|b| b.is_ascii_digit()) {
                self.at += 1;
            }
        }
        // the slice is ASCII digits and signs: valid UTF-8
        let text = String::from_utf8_lossy(&self.s[start..self.at]).into_owned();
        Ok(Json::Num(Num(text)))
    }

    fn hex4(&mut self) -> Result<u32, ReadError> {
        let h = self
            .s
            .get(self.at..self.at + 4)
            .ok_or_else(|| self.err("a \\u escape needs four hex digits"))?;
        let mut v = 0u32;
        for &b in h {
            let d = (b as char)
                .to_digit(16)
                .ok_or_else(|| self.err("a \\u escape needs four hex digits"))?;
            v = v * 16 + d;
        }
        self.at += 4;
        Ok(v)
    }

    fn string(&mut self) -> Result<String, ReadError> {
        self.at += 1; // the opening quote
        let mut out = String::new();
        loop {
            let run = self.at;
            while let Some(&b) = self.s.get(self.at) {
                if b == b'"' || b == b'\\' || b < 0x20 {
                    break;
                }
                self.at += 1;
            }
            // the input is a str and the run stops only at ASCII bytes: a char boundary
            out.push_str(&String::from_utf8_lossy(&self.s[run..self.at]));
            match self.peek() {
                None => return Err(self.err("a string never ends")),
                Some(b'"') => {
                    self.at += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.at += 1;
                    let e = self.peek().ok_or_else(|| self.err("a string never ends"))?;
                    self.at += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            let c = if (0xD800..0xDC00).contains(&hi) {
                                if !self.eat(b"\\u") {
                                    return Err(self.err("a lone surrogate escape"));
                                }
                                let lo = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&lo) {
                                    return Err(self.err("a lone surrogate escape"));
                                }
                                0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                            } else if (0xDC00..0xE000).contains(&hi) {
                                return Err(self.err("a lone surrogate escape"));
                            } else {
                                hi
                            };
                            out.push(
                                char::from_u32(c)
                                    .ok_or_else(|| self.err("an escape that is no character"))?,
                            );
                        }
                        _ => return Err(self.err("an unknown escape")),
                    }
                }
                Some(_) => return Err(self.err("a raw control character in a string")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Result<Json, ReadError> {
        parse(s.as_bytes(), 16, 1000)
    }

    #[test]
    fn reads_values_and_keeps_numbers_as_written() {
        let v =
            p(r#" {"a": [1, -2.50, 3e2, true, null, "x\u00e9\ud83d\ude00"], "b": {}} "#).unwrap();
        let a = v.get("a").and_then(Json::as_arr).unwrap();
        let Json::Num(n) = &a[1] else { panic!() };
        assert_eq!(n.0, "-2.50");
        assert_eq!(n.ratio(), Some((-250, 100)));
        let Json::Num(n) = &a[2] else { panic!() };
        assert_eq!(n.as_i64(), Some(300));
        assert_eq!(a[5].as_str(), Some("x\u{e9}\u{1F600}"));
        assert!(v.get("b").unwrap().is_empty_obj());
    }

    #[test]
    fn reads_the_non_finite_tokens_otio_writes() {
        let v = p("[NaN, Infinity, -Infinity, Inf, -Inf]").unwrap();
        let a = v.as_arr().unwrap();
        let f: Vec<f64> = a
            .iter()
            .map(|x| match x {
                Json::Num(n) => n.as_f64(),
                _ => panic!(),
            })
            .collect();
        assert!(f[0].is_nan());
        assert_eq!(
            &f[1..],
            &[
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY
            ]
        );
        for x in a {
            let Json::Num(n) = x else { panic!() };
            assert_eq!(n.ratio(), None);
        }
    }

    #[test]
    fn refuses_what_is_not_json() {
        for bad in [
            "",
            "{",
            "[1,]",
            "{\"a\":1,}",
            "{\"a\" 1}",
            "01",
            "1.",
            "-",
            "\"\\x\"",
            "\"\u{1}\"",
            "\"\\ud800\"",
            "\"\\udc00\"",
            "{\"a\":1,\"a\":2}",
            "[] []",
            "tru",
            "{1:2}",
        ] {
            assert!(
                matches!(p(bad), Err(ReadError::Syntax { .. })),
                "{bad:?} was read"
            );
        }
        assert!(matches!(
            parse(&[b'"', 0xff, b'"'], 4, 10),
            Err(ReadError::Syntax {
                format: "utf-8",
                ..
            })
        ));
    }

    #[test]
    fn refuses_past_its_limits() {
        let deep = "[".repeat(17) + &"]".repeat(17);
        assert!(matches!(p(&deep), Err(ReadError::Limit { .. })));
        let ok = "[".repeat(16) + &"]".repeat(16);
        assert!(p(&ok).is_ok());
        let many = format!("[{}0]", "0,".repeat(1000));
        assert!(matches!(p(&many), Err(ReadError::Limit { .. })));
    }

    /// Every value's range, read again on its own, is the same value; every key's position is
    /// its opening quote; the spanned and the plain readers agree.
    fn spans_hold(bytes: &[u8]) {
        fn walk(bytes: &[u8], n: &Node) {
            let again = parse_spanned(&bytes[n.start..n.end], 64, 10_000).unwrap();
            assert_eq!(again.to_json(), n.to_json(), "{:?}", &bytes[n.start..n.end]);
            match &n.v {
                NodeValue::Arr(a) => a.iter().for_each(|c| walk(bytes, c)),
                NodeValue::Obj(kv) => {
                    for (k, at, v) in kv {
                        assert_eq!(bytes[*at], b'"');
                        let mut q = Vec::new();
                        write_str(k, &mut q);
                        if !k.contains(['\\', '/']) && k.is_ascii() {
                            assert!(bytes[*at..].starts_with(&q), "key {k}");
                        }
                        walk(bytes, v);
                    }
                }
                _ => {}
            }
        }
        let n = parse_spanned(bytes, 64, 10_000).unwrap();
        assert_eq!(n.to_json(), parse(bytes, 64, 10_000).unwrap());
        walk(bytes, &n);
    }

    #[test]
    fn spans_point_at_each_value_as_written() {
        spans_hold(br#"{"a": [1, -2.50 , 3e2, true, null, "x\u00e9"], "b": {}, "c": {"d": []}}"#);
        spans_hold(b"\xef\xbb\xbf [ {\"k\" : \"v\"} ,\n\t-Inf, NaN ] ");
        let n = parse_spanned(b"\xef\xbb\xbf{\"r\": 29.97}", 8, 100).unwrap();
        assert_eq!(n.start, 3);
        let r = n.get("r").unwrap();
        assert_eq!((r.start, r.end), (9, 14));
        assert_eq!(r.as_num().unwrap().text(), "29.97");
        assert_eq!(quote_str("a\"b\n"), "\"a\\\"b\\u000a\"");
    }

    #[test]
    fn the_spanned_reader_refuses_as_the_plain_one() {
        for bad in ["", "{", "[1,]", "{\"a\":1,\"a\":2}", "[] []", "{1:2}"] {
            assert!(parse_spanned(bad.as_bytes(), 16, 1000).is_err(), "{bad:?}");
        }
        let deep = "[".repeat(17) + &"]".repeat(17);
        assert!(matches!(
            parse_spanned(deep.as_bytes(), 16, 1000),
            Err(ReadError::Limit { .. })
        ));
        let many = format!("[{}0]", "0,".repeat(1000));
        assert!(matches!(
            parse_spanned(many.as_bytes(), 16, 1000),
            Err(ReadError::Limit { .. })
        ));
    }

    #[test]
    fn canonical_form_sorts_keys() {
        let a = p(r#"{"b":1,"a":[true,"\n"]}"#).unwrap();
        let b = p(r#"{ "a" : [ true , "\n" ] , "b" : 1 }"#).unwrap();
        let (mut x, mut y) = (Vec::new(), Vec::new());
        a.canonical(&mut x);
        b.canonical(&mut y);
        assert_eq!(x, y);
        assert_eq!(x, br#"{"a":[true,"\u000a"],"b":1}"#);
    }
}
