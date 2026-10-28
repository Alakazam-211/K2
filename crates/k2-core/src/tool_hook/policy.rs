//! Sew 0.5.0 (`k2_managed_avatar`): K2 owns the person's profile in Sew.
//!
//! Once Sew reports the [`super::features::K2_MANAGED_AVATAR`] flag, K2
//! makes the install's org policy `identity.managed_fields` exactly
//! `["display_name", "title", "pronouns", "avatar"]` (compared **as a
//! set**; order is not significant). With it set, a member can't edit those
//! fields in Sew (their own avatar upload or clear answers 403
//! `policy_denied`, reason `managed_by_provider`), and K2's `force=true`
//! pushes ([`super::avatar`]) are the only picture writer.
//!
//! **Why title and pronouns stay K2's (Sew 2026-10-28, option a).** Sew
//! 0.5.0's `principals/provision` is a declarative upsert: an absent
//! `title` or `pronouns` CLEARS it. If they were the member's own, every
//! re-provision (a guest flip, an assistant re-pin, a reconcile repair)
//! would race the member's edits. So K2 keeps sending them (A8: a provision
//! to an existing member carries the values Sew's member list holds) and
//! keeps them managed. Sew 0.5.1 makes provision leave member-owned fields
//! alone; then K2 can narrow the set to `[display_name, avatar]` and stop
//! sending title/pronouns (a 0.46.7 follow-up, gated on Sew 0.5.1).
//!
//! **Reconcile, never assume.** The sweep reads `GET /mgmt/v1/policies`
//! (`{"policies": {...}}`; an absent key is Sew's default
//! `[display_name, title, pronouns]`) and records what it saw ([`note`]).
//! When the value isn't K2's set, [`plan`] queues one `policy_set` op on
//! the S27 worker (retries, backoff, a stable `Idempotency-Key`, the row
//! is the audit trail):
//!
//! | Op | Sew call | Body |
//! |---|---|---|
//! | `policy_set` | `POST /mgmt/v1/policies` | `{"changes": {"identity.managed_fields": ["display_name", "title", "pronouns", "avatar"]}}` |
//!
//! Sew's answer is `200 {policies}`; the same value again is a no-op there
//! (no audit row), and a duplicate or unknown value is `400
//! validation_failed` (the row blocks loudly). A Sew without the flag (0.3.x)
//! never sees the call. A new install (purge, reinstall) forgets what K2
//! saw, so the next sweep sets it again.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

use super::{features, Cause, NewOp, Op, CHAT_KIND};

/// `GET` (all org policies) and `POST` (`{"changes": {...}}`).
pub const MGMT_POLICIES: &str = "/mgmt/v1/policies";
/// The policy key K2 sets.
pub const MANAGED_FIELDS: &str = "identity.managed_fields";
/// The fields K2 owns in Sew 0.5.0 (option a: title and pronouns stay
/// K2's until Sew 0.5.1's provision leaves member-owned fields alone).
pub const K2_MANAGED_FIELDS: [&str; 4] = ["display_name", "title", "pronouns", "avatar"];
/// Outbox subject of the `policy_set` row.
pub const SUBJECT: &str = "policy:identity.managed_fields";

/// Idempotent DDL, called from `skin::open_db` beside `tool_outbox`.
pub fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tool_sew_policy (
            key       TEXT PRIMARY KEY NOT NULL,
            seen      TEXT,
            is_k2     INTEGER NOT NULL,
            seen_at   INTEGER NOT NULL
         );",
    )
    .map_err(|e| format!("skin.db sew policy schema: {e}"))
}

/// Is `v` (the policy's value as Sew showed it; `None` = absent, Sew's
/// default) exactly K2's set? Order is not significant; a duplicate, or
/// anything that is not a list of strings, is not K2's.
pub fn is_k2_set(v: Option<&Value>) -> bool {
    let Some(Value::Array(items)) = v else { return false };
    let mut have: Vec<&str> = Vec::with_capacity(items.len());
    for i in items {
        let Some(s) = i.as_str() else { return false };
        have.push(s);
    }
    have.sort_unstable();
    have.dedup();
    let mut want: Vec<&str> = K2_MANAGED_FIELDS.to_vec();
    want.sort_unstable();
    have == want && items.len() == want.len()
}

/// The `POST /mgmt/v1/policies` body K2 sends.
pub fn set_body() -> Value {
    json!({ "changes": { MANAGED_FIELDS: K2_MANAGED_FIELDS } })
}

/// The `identity.managed_fields` value in a `{"policies": {...}}` body:
/// `Ok(None)` = absent (Sew's default). `Err` = not a policies body.
pub fn managed_fields_in(body: &[u8]) -> Result<Option<Value>, String> {
    let v: Value = serde_json::from_slice(body).map_err(|e| format!("{MGMT_POLICIES}: unexpected body ({e})"))?;
    let Some(p) = v.get("policies").and_then(Value::as_object) else {
        return Err(format!("{MGMT_POLICIES}: no policies object in the answer"));
    };
    Ok(p.get(MANAGED_FIELDS).filter(|x| !x.is_null()).cloned())
}

/// Record what Sew showed for `identity.managed_fields` (`None` = absent).
pub fn note(conn: &Connection, seen: Option<&Value>, now: i64) -> Result<(), String> {
    let raw = seen.map(Value::to_string);
    conn.execute(
        "INSERT INTO tool_sew_policy (key, seen, is_k2, seen_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(key) DO UPDATE SET seen = excluded.seen, is_k2 = excluded.is_k2, seen_at = excluded.seen_at",
        params![MANAGED_FIELDS, raw, is_k2_set(seen) as i64, now],
    )
    .map(|_| ())
    .map_err(|e| format!("sew policy write: {e}"))
}

/// Did Sew last show K2's set? `None` = K2 hasn't read it on this install.
pub fn seen_is_k2(conn: &Connection) -> Result<Option<bool>, String> {
    conn.query_row("SELECT is_k2 FROM tool_sew_policy WHERE key = ?1", params![MANAGED_FIELDS], |r| r.get::<_, i64>(0))
        .optional()
        .map(|o| o.map(|v| v != 0))
        .map_err(|e| format!("sew policy read: {e}"))
}

/// Sew reports `k2_managed_avatar` AND last showed K2's set.
pub fn is_set(conn: &Connection) -> Result<bool, String> {
    Ok(features::has(conn, CHAT_KIND, features::K2_MANAGED_AVATAR)? && seen_is_k2(conn)? == Some(true))
}

/// The sweep, after [`note`]: one `policy_set` when Sew has the flag, K2
/// has read the policy on this install, and it isn't K2's set (and no
/// `policy_set` row is open).
pub fn plan(conn: &Connection, busy: &std::collections::HashSet<String>) -> Result<Vec<NewOp>, String> {
    if !features::has(conn, CHAT_KIND, features::K2_MANAGED_AVATAR)? || busy.contains(SUBJECT) {
        return Ok(Vec::new());
    }
    if seen_is_k2(conn)? != Some(false) {
        return Ok(Vec::new());
    }
    Ok(vec![NewOp {
        tool: CHAT_KIND,
        op: Op::PolicySet,
        subject_key: SUBJECT.to_string(),
        payload: set_body(),
        grant_id: None,
        grant_rev: None,
        cause: Cause::Reconcile,
        after_subject: None,
    }])
}

/// A delivered `policy_set`: record what Sew's answer (`{policies}`) shows.
/// An answer K2 can't read leaves the record as it was (the next sweep's
/// read decides).
pub fn after_done(conn: &Connection, row_id: &str, result: &str, now: i64) -> Result<(), String> {
    let op: Option<String> = conn
        .query_row("SELECT op FROM tool_outbox WHERE id = ?1", params![row_id], |r| r.get(0))
        .optional()
        .map_err(|e| format!("sew policy row read: {e}"))?;
    if op.as_deref().and_then(Op::parse) != Some(Op::PolicySet) {
        return Ok(());
    }
    match managed_fields_in(result.as_bytes()) {
        Ok(seen) => note(conn, seen.as_ref(), now),
        Err(e) => {
            crate::log_debug!("[grant-hook] policy_set answer unreadable: {e}");
            Ok(())
        }
    }
}

/// A new Sew install: K2 hasn't read its policy yet.
pub fn forget(conn: &Connection) -> Result<(), String> {
    conn.execute("DELETE FROM tool_sew_policy", []).map(|_| ()).map_err(|e| format!("sew policy reset: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn k2s_set_is_compared_as_a_set() {
        assert!(is_k2_set(Some(&json!(["display_name", "title", "pronouns", "avatar"]))));
        assert!(is_k2_set(Some(&json!(["avatar", "pronouns", "display_name", "title"]))), "order is not significant");
        assert!(!is_k2_set(None), "absent = Sew's default, not K2's");
        assert!(!is_k2_set(Some(&json!(["display_name", "title", "pronouns"]))), "Sew's default");
        assert!(!is_k2_set(Some(&json!(["display_name", "avatar"]))), "the 0.5.1 narrowing, not 0.5.0's");
        assert!(!is_k2_set(Some(&json!(["display_name", "title", "pronouns", "avatar", "email"]))));
        assert!(!is_k2_set(Some(&json!(["display_name", "title", "pronouns", "avatar", "avatar"]))), "a duplicate is not the set");
        assert!(!is_k2_set(Some(&json!("display_name,title,pronouns,avatar"))));
        assert!(!is_k2_set(Some(&json!(["display_name", "title", "pronouns", 1]))));
        assert_eq!(
            set_body(),
            json!({"changes": {"identity.managed_fields": ["display_name", "title", "pronouns", "avatar"]}})
        );
    }

    #[test]
    fn the_policy_is_read_from_a_policies_body() {
        let body = br#"{"policies":{"gifs.enabled":true,"identity.managed_fields":["avatar","display_name"]}}"#;
        assert_eq!(managed_fields_in(body).unwrap(), Some(json!(["avatar", "display_name"])));
        assert_eq!(managed_fields_in(br#"{"policies":{"gifs.enabled":true}}"#).unwrap(), None, "absent");
        assert_eq!(managed_fields_in(br#"{"policies":{"identity.managed_fields":null}}"#).unwrap(), None, "reset");
        assert!(managed_fields_in(br#"{"ok":true}"#).is_err());
        assert!(managed_fields_in(b"not json").is_err());
    }
}
