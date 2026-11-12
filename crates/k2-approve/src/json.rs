//! The strict JSON reader every signed approval goes through (SA-4).
//!
//! RFC 8259 syntax, with these extra refusals:
//! - a key repeated in one object (compared after unescaping): `duplicate_key`;
//! - a number with a fraction or exponent, `-0`, or outside ±(2^53 − 1):
//!   `not_integer`;
//! - any forbidden code point in a string or key ([`forbidden`]), raw or
//!   escaped, including a lone surrogate and an unescaped control
//!   character: `forbidden_char`;
//! - anything else that is not JSON, text after the value, or nesting deeper
//!   than [`MAX_DEPTH`]: `json`.
//!
//! Objects keep their keys in the order written; nothing here re-writes bytes.

use crate::{refuse, Code, Result};

/// Deepest nesting read (a payload needs 3: object, body, list).
pub const MAX_DEPTH: usize = 8;
/// The largest whole number allowed: 2^53 − 1, so every reader (Swift's and
/// JavaScript's doubles included) holds it exactly.
pub const MAX_INT: i64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    Arr(Vec<Value>),
    Obj(Vec<(String, Value)>),
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "true/false",
            Value::Int(_) => "a whole number",
            Value::Str(_) => "a string",
            Value::Arr(_) => "a list",
            Value::Obj(_) => "an object",
        }
    }
}

/// True for a code point no signed string may hold: controls, direction
/// marks and overrides, invisible and zero-width characters, fillers,
/// variation selectors, tags, surrogates and noncharacters. The same list is
/// in the wire contract and in every reader.
pub fn forbidden(cp: u32) -> bool {
    matches!(cp,
        0x0000..=0x001F | 0x007F..=0x009F |
        0x00AD | 0x034F | 0x061C | 0x115F | 0x1160 | 0x17B4 | 0x17B5 |
        0x180B..=0x180F |
        0x200B..=0x200F | 0x2028..=0x202E | 0x2060..=0x206F |
        0x3164 | 0xD800..=0xDFFF | 0xFDD0..=0xFDEF |
        0xFE00..=0xFE0F | 0xFEFF | 0xFFA0 | 0xFFF0..=0xFFFB |
        0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A | 0xE0000..=0xE0FFF)
        || (cp & 0xFFFE) == 0xFFFE
}

/// Parse one JSON value from UTF-8 text (already checked to be UTF-8).
pub fn parse(text: &str) -> Result<Value> {
    let mut p = Parser { b: text.as_bytes(), s: text, i: 0 };
    p.ws();
    let v = p.value(0)?;
    p.ws();
    if p.i != p.b.len() {
        return refuse(Code::Json, format!("text after the JSON value at byte {}", p.i));
    }
    Ok(v)
}

struct Parser<'a> {
    b: &'a [u8],
    s: &'a str,
    i: usize,
}

impl<'a> Parser<'a> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn syntax<T>(&self, what: &str) -> Result<T> {
        refuse(Code::Json, format!("{what} at byte {}", self.i))
    }

    fn value(&mut self, depth: usize) -> Result<Value> {
        if depth >= MAX_DEPTH {
            return self.syntax("nested too deep");
        }
        match self.b.get(self.i) {
            None => self.syntax("unexpected end"),
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => Ok(Value::Str(self.string()?)),
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            Some(b'n') => self.literal("null", Value::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => self.syntax("not a JSON value"),
        }
    }

    fn literal(&mut self, word: &str, v: Value) -> Result<Value> {
        if self.b[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(v)
        } else {
            self.syntax("not a JSON value")
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value> {
        self.i += 1;
        let mut out: Vec<(String, Value)> = Vec::new();
        self.ws();
        if self.b.get(self.i) == Some(&b'}') {
            self.i += 1;
            return Ok(Value::Obj(out));
        }
        loop {
            self.ws();
            if self.b.get(self.i) != Some(&b'"') {
                return self.syntax("expected a key");
            }
            let k = self.string()?;
            if out.iter().any(|(e, _)| *e == k) {
                return refuse(Code::DuplicateKey, format!("the key {k:?} appears twice in one object"));
            }
            self.ws();
            if self.b.get(self.i) != Some(&b':') {
                return self.syntax("expected ':'");
            }
            self.i += 1;
            self.ws();
            let v = self.value(depth + 1)?;
            out.push((k, v));
            self.ws();
            match self.b.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Value::Obj(out));
                }
                _ => return self.syntax("expected ',' or '}'"),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value> {
        self.i += 1;
        let mut out = Vec::new();
        self.ws();
        if self.b.get(self.i) == Some(&b']') {
            self.i += 1;
            return Ok(Value::Arr(out));
        }
        loop {
            self.ws();
            out.push(self.value(depth + 1)?);
            self.ws();
            match self.b.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(Value::Arr(out));
                }
                _ => return self.syntax("expected ',' or ']'"),
            }
        }
    }

    fn number(&mut self) -> Result<Value> {
        let start = self.i;
        if self.b[self.i] == b'-' {
            self.i += 1;
        }
        let digits = self.i;
        match self.b.get(self.i) {
            Some(b'0') => self.i += 1,
            Some(b'1'..=b'9') => {
                while matches!(self.b.get(self.i), Some(b'0'..=b'9')) {
                    self.i += 1;
                }
            }
            _ => return self.syntax("a number needs digits"),
        }
        let int_end = self.i;
        let mut whole = true;
        if self.b.get(self.i) == Some(&b'.') {
            self.i += 1;
            if !matches!(self.b.get(self.i), Some(b'0'..=b'9')) {
                return self.syntax("a fraction needs digits");
            }
            while matches!(self.b.get(self.i), Some(b'0'..=b'9')) {
                self.i += 1;
            }
            whole = false;
        }
        if matches!(self.b.get(self.i), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(self.b.get(self.i), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if !matches!(self.b.get(self.i), Some(b'0'..=b'9')) {
                return self.syntax("an exponent needs digits");
            }
            while matches!(self.b.get(self.i), Some(b'0'..=b'9')) {
                self.i += 1;
            }
            whole = false;
        }
        let tok = &self.s[start..self.i];
        if !whole {
            return refuse(Code::NotInteger, format!("{tok} is not a whole number (money is a string: \"125.00 USD\")"));
        }
        if tok == "-0" {
            return refuse(Code::NotInteger, "-0 is not allowed (write 0)");
        }
        let mag = &self.s[digits..int_end];
        if mag.len() > 16 {
            return refuse(Code::NotInteger, format!("{tok} is outside ±(2^53 − 1)"));
        }
        let n: i64 = tok.parse().map_err(|_| crate::Refusal::new(Code::NotInteger, format!("{tok} is not a whole number")))?;
        if !(-MAX_INT..=MAX_INT).contains(&n) {
            return refuse(Code::NotInteger, format!("{tok} is outside ±(2^53 − 1)"));
        }
        Ok(Value::Int(n))
    }

    fn hex4(&mut self) -> Result<u32> {
        let h = self.b.get(self.i..self.i + 4).ok_or_else(|| crate::Refusal::new(Code::Json, "short \\u escape"))?;
        let mut v = 0u32;
        for &c in h {
            let d = (c as char).to_digit(16).ok_or_else(|| crate::Refusal::new(Code::Json, format!("bad \\u escape at byte {}", self.i)))?;
            v = v * 16 + d;
        }
        self.i += 4;
        Ok(v)
    }

    fn push_cp(out: &mut String, cp: u32) -> Result<()> {
        if forbidden(cp) {
            return refuse(Code::ForbiddenChar, format!("a string holds U+{cp:04X}, which is not allowed in a signed approval"));
        }
        match char::from_u32(cp) {
            Some(c) => {
                out.push(c);
                Ok(())
            }
            None => refuse(Code::ForbiddenChar, format!("U+{cp:04X} is not a character")),
        }
    }

    fn string(&mut self) -> Result<String> {
        self.i += 1; // opening quote
        let mut out = String::new();
        loop {
            let Some(&c) = self.b.get(self.i) else {
                return self.syntax("unterminated string");
            };
            match c {
                b'"' => {
                    self.i += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.i += 1;
                    let Some(&e) = self.b.get(self.i) else {
                        return self.syntax("unterminated escape");
                    };
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => Self::push_cp(&mut out, 0x08)?,
                        b'f' => Self::push_cp(&mut out, 0x0C)?,
                        b'n' => Self::push_cp(&mut out, 0x0A)?,
                        b'r' => Self::push_cp(&mut out, 0x0D)?,
                        b't' => Self::push_cp(&mut out, 0x09)?,
                        b'u' => {
                            let hi = self.hex4()?;
                            let cp = if (0xD800..=0xDBFF).contains(&hi)
                                && self.b.get(self.i) == Some(&b'\\')
                                && self.b.get(self.i + 1) == Some(&b'u')
                            {
                                let save = self.i;
                                self.i += 2;
                                let lo = self.hex4()?;
                                if (0xDC00..=0xDFFF).contains(&lo) {
                                    0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                                } else {
                                    // A high surrogate alone: refused below.
                                    self.i = save;
                                    hi
                                }
                            } else {
                                hi
                            };
                            Self::push_cp(&mut out, cp)?;
                        }
                        _ => return self.syntax("unknown escape"),
                    }
                }
                _ => {
                    // One UTF-8 character (the text is valid UTF-8).
                    let ch = self.s[self.i..].chars().next().expect("in bounds");
                    Self::push_cp(&mut out, ch as u32)?;
                    self.i += ch.len_utf8();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(t: &str) -> &'static str {
        parse(t).expect_err("must be refused").code.as_str()
    }

    #[test]
    fn reads_the_allowed_shapes() {
        let v = parse(" {\"a\":[\"x\",\"\\u00e9\\ud83d\\ude00\"],\"b\":-12,\"c\":true,\"d\":{}} ").expect("valid");
        let Value::Obj(o) = v else { panic!("object") };
        assert_eq!(o[0].0, "a");
        assert_eq!(o[0].1, Value::Arr(vec![Value::Str("x".into()), Value::Str("é😀".into())]));
        assert_eq!(o[1].1, Value::Int(-12));
        assert_eq!(parse("9007199254740991").expect("max"), Value::Int(MAX_INT));
        assert_eq!(parse("-9007199254740991").expect("min"), Value::Int(-MAX_INT));
    }

    #[test]
    fn refuses_with_the_contract_codes() {
        assert_eq!(code("{\"a\":1,\"a\":2}"), "duplicate_key");
        assert_eq!(code("{\"nonce\":1,\"\\u006eonce\":2}"), "duplicate_key");
        assert_eq!(code("1.5"), "not_integer");
        assert_eq!(code("1e3"), "not_integer");
        assert_eq!(code("-0"), "not_integer");
        assert_eq!(code("9007199254740992"), "not_integer");
        assert_eq!(code("123456789012345678901234567890"), "not_integer");
        assert_eq!(code("\"a\\u202eb\""), "forbidden_char");
        assert_eq!(code("\"a\u{202e}b\""), "forbidden_char");
        assert_eq!(code("\"a\u{200b}b\""), "forbidden_char");
        assert_eq!(code("\"a\\nb\""), "forbidden_char");
        assert_eq!(code("\"a\nb\""), "forbidden_char");
        assert_eq!(code("\"\\ud800\""), "forbidden_char");
        assert_eq!(code("\"\\ud800\\u0041\""), "forbidden_char");
        assert_eq!(code("\"\\udc00\""), "forbidden_char");
        assert_eq!(code("\"\u{fe0f}\""), "forbidden_char");
        assert_eq!(code("\"\u{e0041}\""), "forbidden_char");
        assert_eq!(code("\"\u{ffff}\""), "forbidden_char");
        assert_eq!(code("{} x"), "json");
        assert_eq!(code("{"), "json");
        assert_eq!(code("01"), "json");
        assert_eq!(code("NaN"), "json");
        assert_eq!(code("\u{feff}{}"), "json");
        assert_eq!(code("\"\\x\""), "json");
        assert_eq!(code("[[[[[[[[[]]]]]]]]]"), "json");
    }
}
