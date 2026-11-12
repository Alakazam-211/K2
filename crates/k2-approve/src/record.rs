//! The approval record a device sends back (PRD §6):
//!
//! `{"v":1,"payload_b64":"…","key_id":"…","alg":"ES256","sig_b64":"…"}`
//!
//! Exactly these five fields, strict JSON, standard base64 with padding.
//! It carries no claim about the check or the view (SA-5).

use crate::json::{self, Value};
use crate::{refuse, Alg, Code, Refusal, Result, MAX_PAYLOAD, MAX_RECORD};
use base64::Engine;

pub const RECORD_FIELDS: [&str; 5] = ["v", "payload_b64", "key_id", "alg", "sig_b64"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub payload: Vec<u8>,
    pub key_id: String,
    pub alg: Alg,
    pub sig: Vec<u8>,
}

fn b64(field: &str, s: &str, max: usize) -> Result<Vec<u8>> {
    let e = base64::engine::general_purpose::STANDARD;
    let bytes = e.decode(s).map_err(|_| Refusal::new(Code::Record, format!("{field} is not standard base64")))?;
    // One spelling only: re-encoding gives the same text.
    if e.encode(&bytes) != s {
        return refuse(Code::Record, format!("{field} is not in standard base64 form"));
    }
    if bytes.is_empty() || bytes.len() > max {
        return refuse(Code::Record, format!("{field} is empty or too long"));
    }
    Ok(bytes)
}

pub fn parse(bytes: &[u8]) -> Result<Record> {
    let bad = |d: String| -> Result<Record> { refuse(Code::Record, d) };
    if bytes.len() > MAX_RECORD {
        return bad(format!("the record is over {MAX_RECORD} bytes"));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| Refusal::new(Code::Record, "the record is not UTF-8"))?;
    let v = json::parse(text).map_err(|e| Refusal::new(Code::Record, format!("the record is not strict JSON ({})", e)))?;
    let Value::Obj(o) = v else {
        return bad("the record is not a JSON object".into());
    };
    for (k, _) in &o {
        if !RECORD_FIELDS.contains(&k.as_str()) {
            return bad(format!("unknown record field {k:?}"));
        }
    }
    let get = |f: &str| o.iter().find(|(k, _)| k == f).map(|(_, v)| v);
    let s = |f: &str| match get(f) {
        Some(Value::Str(s)) => Ok(s.clone()),
        _ => Err(Refusal::new(Code::Record, format!("the record's {f} is missing or not a string"))),
    };
    if get("v") != Some(&Value::Int(1)) {
        return bad("the record's v must be 1".into());
    }
    let payload = b64("payload_b64", &s("payload_b64")?, MAX_PAYLOAD)?;
    let key_id = s("key_id")?;
    if !crate::payload::is_hex_lower(&key_id, 64) {
        return bad("the record's key_id is not 64 lowercase hex".into());
    }
    let alg_s = s("alg")?;
    let alg = Alg::parse(&alg_s).ok_or_else(|| Refusal::new(Code::Record, format!("unknown alg {alg_s:?}")))?;
    let sig = b64("sig_b64", &s("sig_b64")?, 1024)?;
    Ok(Record { payload, key_id, alg, sig })
}

/// Write a record (devices, tests). Field order as in the contract.
pub fn to_json(r: &Record) -> String {
    let e = base64::engine::general_purpose::STANDARD;
    format!(
        "{{\"v\":1,\"payload_b64\":\"{}\",\"key_id\":\"{}\",\"alg\":\"{}\",\"sig_b64\":\"{}\"}}",
        e.encode(&r.payload),
        r.key_id,
        r.alg.as_str(),
        e.encode(&r.sig)
    )
}
