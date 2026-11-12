//! What the person sees, worked out from the bytes alone (PRD §5).
//!
//! The phone's native approval screen must produce exactly these lines for
//! every valid fixture (`fixtures/sa1/payloads/valid/*.txt`). The device adds
//! its own line about which device and check will sign; that line is not
//! part of the shared text.

use crate::payload::{BodyValue, Payload};
use crate::time::display_utc;

/// The line every kind without built-in wording starts with.
pub const UNKNOWN_KIND_LINE: &str = "K2 has no built-in wording for this kind. Every field is shown below.";

/// The screen text, one line per entry. Lines are joined with "\n".
pub fn lines(p: &Payload) -> Vec<String> {
    let mut out = Vec::new();
    match p.kind.as_str() {
        "k2.release" => release(p, &mut out),
        _ => generic(p, &mut out),
    }
    out.push(String::new());
    out.push(format!("Who will act: {}", p.rp));
    out.push(format!("Asked: {}", p.approver));
    out.push(format!("Answer by: {}", display_utc(&p.answer_by)));
    out
}

/// The screen text as one string ending in a newline.
pub fn text(p: &Payload) -> String {
    let mut s = lines(p).join("\n");
    s.push('\n');
    s
}

fn release(p: &Payload, out: &mut Vec<String>) {
    // Shape is checked by payload::check_known before anything is shown.
    let s = |f: &str| match p.body_field(f) {
        Some(BodyValue::Str(s)) => s.clone(),
        _ => String::new(),
    };
    let yes = |f: &str| matches!(p.body_field(f), Some(BodyValue::Bool(true)));
    let assets = match p.body_field("assets") {
        Some(BodyValue::List(l)) => l.join(", "),
        _ => String::new(),
    };
    let re = if yes("reapproval") { "RE-APPROVE: " } else { "" };
    out.push(format!("{re}Approve K2 release {} at base commit {}.", s("version"), s("base_commit")));
    out.push(format!("Sign: {assets}."));
    out.push(if yes("windows") { "Windows setup: yes, with no independent check.".to_string() } else { "Windows setup: no.".to_string() });
    out.push(format!("Signing stays open until {}.", display_utc(&s("session_until"))));
}

fn generic(p: &Payload, out: &mut Vec<String>) {
    out.push(UNKNOWN_KIND_LINE.to_string());
    out.push(format!("Kind: {} (version {})", p.kind, p.kind_v));
    let mut fields: Vec<&(String, BodyValue)> = p.body.iter().collect();
    // Sorted by name (byte order): readers that lose the written order agree.
    fields.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    for (k, v) in fields {
        match v {
            BodyValue::Str(s) => out.push(format!("{k}: {s}")),
            BodyValue::Int(n) => out.push(format!("{k}: {n}")),
            BodyValue::Bool(b) => out.push(format!("{k}: {b}")),
            BodyValue::List(l) if l.is_empty() => out.push(format!("{k}: (none)")),
            BodyValue::List(l) => {
                out.push(format!("{k}:"));
                for it in l {
                    out.push(format!("  - {it}"));
                }
            }
        }
    }
}
