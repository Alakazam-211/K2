//! A draft save asked twice answers one draft (0.46.17).
//!
//! A linked draft save can take longer than the CLI waits (IMAP login, the
//! source fetch, a folder survey, the APPEND), and the daemon finishes it
//! anyway. Before this, the CLI said "timeout", the agent tried again, and
//! the mailbox got a second copy. Now every save carries an idempotency key
//! (`idempotencyKey`; the CLI makes one, or the agent passes
//! `--idempotency-key`). The key is stored on the draft's record
//! (`mail_drafts.client_key`, 0321, unique per workspace):
//!
//! * the same key again, once the first save landed: the same draft back
//!   (`replayed: true`), nothing saved twice;
//! * the same key while the first save is still running: 409 `busy`; ask
//!   again shortly with the same key.
//!
//! The key holds no mail text and is never logged with any.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use crate::cli_response::CliResponse;
use crate::mail::drafts::{self, DraftRow};

fn error_response(status: &'static str, code: &str, hint: &str) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({ "ok": false, "error": { "code": code, "hint": hint } }).to_string(),
    }
}

/// Is `key` a usable idempotency key: 8 to 128 of `A-Z a-z 0-9 . _ : -`.
pub fn valid_key(key: &str) -> bool {
    (8..=128).contains(&key.len())
        && key.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
}

/// The usage refusal for a bad key.
pub fn bad_key() -> CliResponse {
    error_response(
        "400 Bad Request",
        "usage",
        "invalid 'idempotencyKey' — 8 to 128 characters of letters, digits, '.', '_', ':' or '-'",
    )
}

/// The answer for a key that already saved a draft: that draft, once.
pub fn replay_answer(row: &DraftRow) -> CliResponse {
    let mut v = serde_json::json!({
        "ok": true,
        "address": row.address,
        "draftId": row.id,
        "kind": row.kind,
        "status": drafts::status_wire(row),
        "replayed": true,
        "hint": format!(
            "draft {} was already saved in {} with this key — nothing was saved twice",
            row.id, row.address
        ),
    });
    if row.kind == drafts::KIND_REPLY {
        v["id"] = serde_json::json!(row.reply_of);
    }
    CliResponse {
        status: "200 OK",
        content_type: "application/json",
        body: v.to_string(),
    }
}

fn running() -> &'static Mutex<HashSet<(String, String)>> {
    static RUNNING: OnceLock<Mutex<HashSet<(String, String)>>> = OnceLock::new();
    RUNNING.get_or_init(|| Mutex::new(HashSet::new()))
}

/// One running save for `(workspace, key)`. Held until the save answers
/// (dropped on every return path).
pub struct KeySlot {
    key: (String, String),
}

impl KeySlot {
    /// Take the slot, or answer 409 `busy` when a save with this key is
    /// still running.
    pub fn try_acquire(project_id: &str, key: &str) -> Result<Self, CliResponse> {
        let k = (project_id.to_string(), key.to_string());
        let mut set = running().lock().unwrap_or_else(|p| p.into_inner());
        if !set.insert(k.clone()) {
            return Err(error_response(
                "409 Conflict",
                "busy",
                "a draft save with this idempotency key is still running (the daemon keeps going \
                 after the CLI stops waiting) — ask again with the same key in a few seconds: it \
                 answers that draft once it is saved, and saves nothing twice",
            ));
        }
        Ok(Self { key: k })
    }
}

impl Drop for KeySlot {
    fn drop(&mut self) {
        running().lock().unwrap_or_else(|p| p.into_inner()).remove(&self.key);
    }
}

/// The start of a save: `Ok(Some(slot))` = go ahead, keep the slot until
/// the save answers; `Ok(None)` = no key, go ahead; `Err(resp)` = answer
/// this instead (a replay, busy, or a bad key).
pub fn begin(project_id: &str, key: &str) -> Result<Option<KeySlot>, CliResponse> {
    if key.is_empty() {
        return Ok(None);
    }
    if !valid_key(key) {
        return Err(bad_key());
    }
    if let Some(row) = drafts::find_by_key(project_id, key) {
        return Err(replay_answer(&row));
    }
    let slot = KeySlot::try_acquire(project_id, key)?;
    // A save that finished between the lookup and the slot.
    if let Some(row) = drafts::find_by_key(project_id, key) {
        return Err(replay_answer(&row));
    }
    Ok(Some(slot))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_short_plain_tokens() {
        assert!(valid_key("k2d_0123456789abcdef"));
        assert!(valid_key("a.b_c:d-e1"));
        assert!(!valid_key("short"));
        assert!(!valid_key(&"x".repeat(129)));
        assert!(!valid_key("has space in it"));
        assert!(!valid_key("new\nline-key"));
    }

    #[test]
    fn a_running_key_is_busy_until_its_slot_drops() {
        let p = format!("p-{}", uuid::Uuid::new_v4().simple());
        let first = KeySlot::try_acquire(&p, "key-00000001").expect("first");
        let busy = KeySlot::try_acquire(&p, "key-00000001").err().expect("second is busy");
        assert_eq!(busy.status, "409 Conflict");
        assert!(busy.body.contains("\"busy\""), "{}", busy.body);
        // Another workspace with the same key is its own.
        let other = KeySlot::try_acquire(&format!("{p}-b"), "key-00000001").expect("other workspace");
        drop(first);
        drop(other);
        let _again = KeySlot::try_acquire(&p, "key-00000001").expect("free after drop");
    }
}
