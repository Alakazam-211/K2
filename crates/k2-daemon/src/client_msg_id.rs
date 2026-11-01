//! post-idempotency-v1 (0.46.8; `.k2/prds/vslive-phone-speed-b1-b2-rev-v1.md`
//! §4): an optional `clientMsgId` on Thread posts (`POST /cli/thread/post`)
//! and project chat posts (`POST /cli/project-group/msg`, and an App
//! guest's `/cli/skin/projects/post`), so a phone that retries a send it
//! never got an answer for doesn't post twice.
//!
//! - The id is 1–64 of `[A-Za-z0-9_-]`, else 400 `usage`, checked before
//!   anything is resolved or stored.
//! - The key is (sender namespace, id). The namespace comes from the
//!   caller's credential, never the body: `p:<person>` (the owner or a
//!   Connect login posting as themselves), `g:<principal>` (an App guest),
//!   `a:<workspace>` (an agent passport), `t:owner` (an owner-token shell
//!   post). It leaves the conversation out: a Thread's key can move
//!   (adoption, a split), and a retry after a move must still find its post.
//! - Thread keys live in redb, written in the post's own transaction; project
//!   chat keys in SQLite (migration 0258), in the post's own savepoint. Both
//!   are kept at least 24 h; the daemon's hourly sweep ([`sweep_if_due`],
//!   from the card-expiry loop, never a client) drops older ones.
//! - A repeat in the same chat answers the stored item with
//!   `"duplicate": true` (plus `"mismatch": true` when the body differs; the
//!   first write wins) and runs no side effect again (no frame, inject,
//!   push, cursor advance or compose history). It does not carry the first
//!   answer's delivery fields (`delivered`, `deliveryReason`,
//!   `deliveredSessionId`, `contextRefused`): those are not stored.
//! - A key whose item is in another chat answers 409 `client_msg_id_reused`.
//!
//! Known gap (accepted by K2 Companion): the post and its key commit
//! together, but the agent inject / push after it are not recorded per
//! post. If the daemon dies between the commit and the inject, the retry
//! gets `duplicate: true` and the agent never gets the line. Re-delivering
//! on a duplicate would need a per-post delivery record (none exists today)
//! and would double-inject on every ordinary retry, so it is not done.
//! Before this feature the same crash meant a double post instead.

use std::sync::atomic::{AtomicI64, Ordering};

use crate::cli_response::CliResponse;

/// The boot-status feature key.
pub const FEATURE: &str = "post-idempotency-v1";

/// The body / query field.
pub const PARAM: &str = "clientMsgId";

/// How long a key is kept at least.
pub const TTL_MS: i64 = k2_core::project_groups::CLIENT_KEY_TTL_MS;

/// How often the daemon sweeps.
pub const SWEEP_EVERY_MS: i64 = 3_600_000;

const MAX_LEN: usize = 64;

/// True for 1–64 chars of `[A-Za-z0-9_-]`.
pub fn is_valid(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_LEN && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn usage(hint: &str) -> CliResponse {
    CliResponse {
        status: "400 Bad Request",
        content_type: "application/json",
        body: serde_json::json!({ "ok": false, "error": { "code": "usage", "hint": hint } }).to_string(),
    }
}

/// The refusal for an id that isn't 1–64 of `[A-Za-z0-9_-]`.
pub fn invalid() -> CliResponse {
    usage("clientMsgId must be 1-64 characters of A-Z, a-z, 0-9, '-' or '_'")
}

/// `Ok(None)` when the post carries no id (old clients: unchanged), the id
/// when it is well formed, else the 400.
pub fn parse(raw: Option<&str>) -> Result<Option<String>, CliResponse> {
    match raw {
        None => Ok(None),
        Some(id) if is_valid(id) => Ok(Some(id.to_string())),
        Some(_) => Err(invalid()),
    }
}

/// 409: the key already names a post in another chat.
pub fn reused() -> CliResponse {
    CliResponse {
        status: "409 Conflict",
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": {
                "code": "client_msg_id_reused",
                "hint": "this clientMsgId already names a post in another chat; send a new id for a new post",
            }
        })
        .to_string(),
    }
}

/// The person key for a login (`owner` for the owner token and linked
/// logins), as read state and push use it.
pub fn person_ns(login: &str) -> String {
    let login = login.trim();
    let login = if login.is_empty() { "owner" } else { login };
    let key = match k2_core::notify_prefs::People::load() {
        Ok(people) => people.person_key(login),
        Err(_) => login.to_ascii_lowercase(),
    };
    format!("p:{key}")
}

/// An App guest's namespace: its login's principal, or (a platform pass
/// with none) the pass itself.
pub fn guest_ns(pass: &k2_core::skin::SkinPass) -> String {
    match pass.principal_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(p) => format!("g:{p}"),
        None => format!("g:pass:{}", pass.id),
    }
}

/// An agent passport's namespace.
pub fn agent_ns(workspace_uuid: &str) -> String {
    format!("a:{}", workspace_uuid.trim())
}

/// The owner token posting from a shell (not as the person's compose bar).
pub const OWNER_SHELL_NS: &str = "t:owner";

/// What a Thread post sent, hashed (a repeat whose body differs answers
/// `mismatch`): the text and the attachment request as sent.
pub fn body_hash(text: &str, attachments: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(text.as_bytes());
    h.update([0u8]);
    h.update(attachments.trim().as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// One sweep at `now_ms`: drop Thread and project chat keys older than
/// [`TTL_MS`]. Returns (thread, project) counts.
pub fn sweep(now_ms: i64) -> (usize, usize) {
    let before = now_ms - TTL_MS;
    let thread = match k2_core::overlay::sweep_client_msg_ids(before) {
        Ok(n) => n,
        Err(e) => {
            k2_core::log_debug!("[client_msg_id] Thread key sweep: {e}");
            0
        }
    };
    let project = match k2_core::project_groups::sweep_client_keys(before) {
        Ok(n) => n,
        Err(e) => {
            k2_core::log_debug!("[client_msg_id] project key sweep: {e}");
            0
        }
    };
    (thread, project)
}

static LAST_SWEEP_MS: AtomicI64 = AtomicI64::new(i64::MIN);

/// The hourly gate: never swept yet, or an hour since the last sweep.
fn due(last_ms: i64, now_ms: i64) -> bool {
    last_ms == i64::MIN || now_ms.saturating_sub(last_ms) >= SWEEP_EVERY_MS
}

/// [`sweep`] at most once per [`SWEEP_EVERY_MS`] (called from the daemon's
/// one-minute card-expiry loop; the first call sweeps). So a key lives
/// 24–25 h. Returns whether it swept.
pub fn sweep_if_due(now_ms: i64) -> bool {
    let last = LAST_SWEEP_MS.load(Ordering::SeqCst);
    if !due(last, now_ms) {
        return false;
    }
    if LAST_SWEEP_MS.compare_exchange(last, now_ms, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        return false;
    }
    sweep(now_ms);
    true
}

/// Tests that write client keys hold this: a sweep test drops every key
/// older than its cutoff, which must not race another test's repeat.
#[cfg(test)]
pub(crate) fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_1_to_64_of_the_safe_set() {
        for ok in ["a", "A-z_0", &"x".repeat(64), "0123456789abcdefABCDEF-_"] {
            assert!(is_valid(ok), "{ok:?} must be valid");
            assert_eq!(parse(Some(ok)).ok().flatten().as_deref(), Some(ok));
        }
        for bad in ["", &"x".repeat(65), "a b", "a/b", "a.b", "é", "a\u{0}", "a+b", " a"] {
            assert!(!is_valid(bad), "{bad:?} must be refused");
            let r = parse(Some(bad)).expect_err("refused");
            assert_eq!(r.status, "400 Bad Request");
            let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
            assert_eq!(v["error"]["code"], "usage", "{v}");
        }
        assert!(matches!(parse(None), Ok(None)), "no id: unchanged");
    }

    #[test]
    fn reused_is_a_409_with_its_code() {
        let r = reused();
        assert_eq!(r.status, "409 Conflict");
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["error"]["code"], "client_msg_id_reused");
    }

    #[test]
    fn body_hash_follows_text_and_files() {
        assert_eq!(body_hash("hi", ""), body_hash("hi", "  "));
        assert_ne!(body_hash("hi", ""), body_hash("hi!", ""));
        assert_ne!(body_hash("hi", ""), body_hash("hi", "[{\"path\":\"a\"}]"));
        assert_ne!(body_hash("a", "b"), body_hash("ab", ""), "the separator keeps text and files apart");
    }

    #[test]
    fn hourly_gate_sweeps_once_per_hour() {
        let t = 1_800_000_000_000i64;
        assert!(due(i64::MIN, t), "the first call sweeps");
        assert!(!due(t, t + 59 * 60_000), "within the hour: no sweep");
        assert!(due(t, t + SWEEP_EVERY_MS), "an hour later: sweep");
        assert!(!due(t, t - 1), "a clock step back never sweeps early");
    }
}
