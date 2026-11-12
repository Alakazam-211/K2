//! The payload: `K2-SIGNED-APPROVAL-1` 0x00 <JSON text> (PRD §5).
//!
//! Every field is checked here, in a fixed order, and a payload that breaks
//! any rule is refused, never shown. Nobody re-writes the bytes: a device
//! signs what it received and a relying party verifies what it stored.

use crate::json::{self, Value};
use crate::{refuse, time, Code, Refusal, Result, MAX_PAYLOAD, PREFIX};

/// The top-level fields, in the order they are checked.
pub const FIELDS: [&str; 9] = ["kind", "kind_v", "rp", "approver", "approver_keys", "nonce", "issued", "answer_by", "body"];
/// At most this many accepted keys in `approver_keys`.
pub const MAX_APPROVER_KEYS: usize = 16;
/// At most this many fields in `body`, and items in one body list.
pub const MAX_BODY_FIELDS: usize = 64;
pub const MAX_LIST_ITEMS: usize = 256;
/// `rp` and `approver` are 1 to this many bytes of UTF-8.
pub const MAX_NAME_BYTES: usize = 128;

/// One `body` value: flat by rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyValue {
    Str(String),
    Int(i64),
    Bool(bool),
    List(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payload {
    /// The exact bytes, prefix included: what is signed and verified.
    pub bytes: Vec<u8>,
    pub kind: String,
    pub kind_v: i64,
    pub rp: String,
    pub approver: String,
    pub approver_keys: Vec<String>,
    pub nonce: String,
    pub issued: String,
    pub issued_at: i64,
    pub answer_by: String,
    pub answer_by_at: i64,
    /// In the order written in the bytes.
    pub body: Vec<(String, BodyValue)>,
}

impl Payload {
    pub fn body_field(&self, name: &str) -> Option<&BodyValue> {
        self.body.iter().find(|(k, _)| k == name).map(|(_, v)| v)
    }
}

pub(crate) fn is_hex_lower(s: &str, n: usize) -> bool {
    s.len() == n && s.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

fn kind_ok(s: &str) -> bool {
    // ^[a-z0-9]+(\.[a-z0-9-]+)+$, at most 64 bytes.
    if s.is_empty() || s.len() > 64 {
        return false;
    }
    let mut parts = s.split('.');
    let first = parts.next().unwrap_or_default();
    if first.is_empty() || !first.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()) {
        return false;
    }
    let mut more = 0;
    for p in parts {
        if p.is_empty() || !p.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-') {
            return false;
        }
        more += 1;
    }
    more >= 1
}

fn body_key_ok(s: &str) -> bool {
    // ^[a-z][a-z0-9_]{0,63}$
    let b = s.as_bytes();
    !b.is_empty() && b.len() <= 64 && b[0].is_ascii_lowercase() && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}

fn want_str<'v>(name: &str, v: &'v Value) -> Result<&'v str> {
    match v {
        Value::Str(s) => Ok(s),
        other => refuse(Code::FieldType, format!("{name} must be a string, not {}", other.type_name())),
    }
}

fn want_name(name: &str, v: &Value) -> Result<String> {
    let s = want_str(name, v)?;
    if s.is_empty() || s.len() > MAX_NAME_BYTES {
        return refuse(Code::FieldFormat, format!("{name} must be 1 to {MAX_NAME_BYTES} bytes"));
    }
    Ok(s.to_string())
}

fn want_time(name: &str, v: &Value) -> Result<(String, i64)> {
    let s = want_str(name, v)?;
    match time::parse_utc(s) {
        Some(t) => Ok((s.to_string(), t)),
        None => refuse(Code::FieldFormat, format!("{name} must be a UTC time like 2026-11-11T14:00:00Z, not {s:?}")),
    }
}

/// Check the bytes and the strict form, then every field. The known kinds'
/// own shapes are checked too ([`check_known`]).
pub fn parse(bytes: &[u8]) -> Result<Payload> {
    if bytes.len() > MAX_PAYLOAD {
        return refuse(Code::TooLarge, format!("{} bytes; a payload is at most {MAX_PAYLOAD}", bytes.len()));
    }
    let Some(rest) = bytes.strip_prefix(PREFIX) else {
        return refuse(Code::Prefix, "the bytes do not start with K2-SIGNED-APPROVAL-1 and a zero byte");
    };
    let text = std::str::from_utf8(rest).map_err(|e| Refusal::new(Code::Utf8, format!("not UTF-8 at byte {}", PREFIX.len() + e.valid_up_to())))?;
    let Value::Obj(top) = json::parse(text)? else {
        return refuse(Code::NotObject, "the JSON is not one object");
    };
    for (k, _) in &top {
        if !FIELDS.contains(&k.as_str()) {
            return refuse(Code::UnknownField, format!("unknown field {k:?}"));
        }
    }
    for f in FIELDS {
        if !top.iter().any(|(k, _)| k == f) {
            return refuse(Code::MissingField, format!("missing field {f}"));
        }
    }
    let get = |f: &str| -> &Value { &top.iter().find(|(k, _)| k == f).expect("checked present").1 };

    let kind = want_str("kind", get("kind"))?.to_string();
    if !kind_ok(&kind) {
        return refuse(Code::FieldFormat, format!("kind {kind:?} must look like k2.release or com.example.refund"));
    }
    let kind_v = match get("kind_v") {
        Value::Int(n) => *n,
        other => return refuse(Code::FieldType, format!("kind_v must be a whole number, not {}", other.type_name())),
    };
    if !(1..=65535).contains(&kind_v) {
        return refuse(Code::FieldFormat, "kind_v must be 1 to 65535");
    }
    let rp = want_name("rp", get("rp"))?;
    let approver = want_name("approver", get("approver"))?;
    let approver_keys = match get("approver_keys") {
        Value::Arr(items) => {
            let mut out: Vec<String> = Vec::new();
            for it in items {
                let Value::Str(s) = it else {
                    return refuse(Code::FieldType, format!("approver_keys holds {}, not a key id", it.type_name()));
                };
                if !is_hex_lower(s, 64) {
                    return refuse(Code::FieldFormat, format!("approver_keys: {s:?} is not a key id (64 lowercase hex)"));
                }
                if out.contains(s) {
                    return refuse(Code::FieldFormat, format!("approver_keys lists {s} twice"));
                }
                out.push(s.clone());
            }
            if out.is_empty() || out.len() > MAX_APPROVER_KEYS {
                return refuse(Code::FieldFormat, format!("approver_keys must list 1 to {MAX_APPROVER_KEYS} keys"));
            }
            out
        }
        other => return refuse(Code::FieldType, format!("approver_keys must be a list, not {}", other.type_name())),
    };
    let nonce = want_str("nonce", get("nonce"))?.to_string();
    if !is_hex_lower(&nonce, 32) {
        return refuse(Code::FieldFormat, "nonce must be 32 lowercase hex");
    }
    let (issued, issued_at) = want_time("issued", get("issued"))?;
    let (answer_by, answer_by_at) = want_time("answer_by", get("answer_by"))?;
    let body = match get("body") {
        Value::Obj(fields) => {
            if fields.len() > MAX_BODY_FIELDS {
                return refuse(Code::FieldFormat, format!("body has more than {MAX_BODY_FIELDS} fields"));
            }
            let mut out = Vec::new();
            for (k, v) in fields {
                if !body_key_ok(k) {
                    return refuse(Code::FieldFormat, format!("body field name {k:?} must be lowercase letters, digits and _ (starting with a letter)"));
                }
                let bv = match v {
                    Value::Str(s) => BodyValue::Str(s.clone()),
                    Value::Int(n) => BodyValue::Int(*n),
                    Value::Bool(b) => BodyValue::Bool(*b),
                    Value::Arr(items) => {
                        let mut l = Vec::new();
                        for it in items {
                            let Value::Str(s) = it else {
                                return refuse(Code::BodyValue, format!("body.{k} lists {}; a body list holds strings only", it.type_name()));
                            };
                            l.push(s.clone());
                        }
                        if l.len() > MAX_LIST_ITEMS {
                            return refuse(Code::FieldFormat, format!("body.{k} lists more than {MAX_LIST_ITEMS} items"));
                        }
                        BodyValue::List(l)
                    }
                    other => return refuse(Code::BodyValue, format!("body.{k} is {}; body is flat (string, whole number, true/false, list of strings)", other.type_name())),
                };
                out.push((k.clone(), bv));
            }
            out
        }
        other => return refuse(Code::FieldType, format!("body must be an object, not {}", other.type_name())),
    };
    if answer_by_at <= issued_at {
        return refuse(Code::Times, "answer_by must be after issued");
    }
    let p = Payload { bytes: bytes.to_vec(), kind, kind_v, rp, approver, approver_keys, nonce, issued, issued_at, answer_by, answer_by_at, body };
    check_known(&p)?;
    Ok(p)
}

/// The kinds K2 has built-in wording for. A known kind whose body does not
/// match its shape exactly is refused (`shape`); any other kind is shown as
/// its full field list.
pub const KNOWN_KINDS: [&str; 1] = ["k2.release"];

/// k2.release v1's body, in display order.
pub const RELEASE_FIELDS: [&str; 8] = ["version", "base_commit", "assets", "slots", "windows", "reapproval", "max_signatures", "session_until"];

fn version_ok(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.len() <= 6 && p.bytes().all(|c| c.is_ascii_digit()) && (p.len() == 1 || !p.starts_with('0')))
}

/// A K2 release file name: ^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$ (k2sign's ASSET_RE).
pub fn asset_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b.len() <= 128 && b[0].is_ascii_alphanumeric() && b.iter().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

pub fn check_known(p: &Payload) -> Result<()> {
    if p.kind != "k2.release" {
        return Ok(());
    }
    let shape = |d: String| -> Result<()> { refuse(Code::Shape, format!("k2.release: {d}")) };
    if p.kind_v != 1 {
        return shape(format!("version {} is not one this reader knows (1)", p.kind_v));
    }
    for (k, _) in &p.body {
        if !RELEASE_FIELDS.contains(&k.as_str()) {
            return shape(format!("unexpected body field {k}"));
        }
    }
    for f in RELEASE_FIELDS {
        if p.body_field(f).is_none() {
            return shape(format!("body field {f} is missing"));
        }
    }
    let s = |f: &str| match p.body_field(f) {
        Some(BodyValue::Str(s)) => Some(s.as_str()),
        _ => None,
    };
    let list = |f: &str| match p.body_field(f) {
        Some(BodyValue::List(l)) => Some(l),
        _ => None,
    };
    let boolean = |f: &str| matches!(p.body_field(f), Some(BodyValue::Bool(_)));
    if !s("version").is_some_and(version_ok) {
        return shape("version must be like 0.46.13".into());
    }
    if !s("base_commit").is_some_and(|b| is_hex_lower(b, 40)) {
        return shape("base_commit must be 40 lowercase hex".into());
    }
    match list("assets") {
        Some(a) if !a.is_empty() && a.iter().all(|x| asset_ok(x)) && a.iter().enumerate().all(|(i, x)| !a[..i].contains(x)) => {}
        _ => return shape("assets must list one or more file names, each once".into()),
    }
    match list("slots") {
        Some(sl) if !sl.is_empty() && sl.iter().all(|x| !x.is_empty() && x.len() <= 8 && x.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())) => {}
        _ => return shape("slots must list one or more key slots".into()),
    }
    if !boolean("windows") || !boolean("reapproval") {
        return shape("windows and reapproval must be true or false".into());
    }
    match p.body_field("max_signatures") {
        Some(BodyValue::Int(n)) if (1..=100_000).contains(n) => {}
        _ => return shape("max_signatures must be a whole number from 1".into()),
    }
    if !s("session_until").is_some_and(|t| time::parse_utc(t).is_some()) {
        return shape("session_until must be a UTC time".into());
    }
    Ok(())
}
