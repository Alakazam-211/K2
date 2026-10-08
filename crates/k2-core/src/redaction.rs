//! Exact-value redaction for text K2 itself stores (K2 vault KV33): Thread
//! posts, tickets, the activity feed, compose send history and inbox
//! notes. The daemon installs the vault's matcher at boot; every active
//! `user` secret (8+ bytes, plus its base64/hex/percent/JSON forms) is
//! replaced by `[k2v:NAME]`. A backstop, not the control: values are
//! meant to stay out of these places in the first place. Not applied to
//! terminal scrollback (memory only) or vendor transcripts.

use std::sync::OnceLock;

type Hook = Box<dyn Fn(&str) -> String + Send + Sync>;

static HOOK: OnceLock<Hook> = OnceLock::new();

pub fn install(f: Hook) -> bool {
    HOOK.set(f).is_ok()
}

/// `s` with known secret values replaced (unchanged without a hook).
pub fn redact(s: &str) -> String {
    match HOOK.get() {
        Some(f) if !s.is_empty() => f(s),
        _ => s.to_string(),
    }
}

pub fn redact_opt(s: Option<&str>) -> Option<String> {
    s.map(redact)
}
