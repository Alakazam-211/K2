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

/// Sew 0.10.0 (`agents_dm_with_box_people`): the install policy that lets
/// an agent open a DM with a box person (`k2:owner`, `k2:connect:`). K2
/// sets it **false** once Sew reports the flag (prd-home-people-channels-v1
/// §13.2, HP-31): Sew chat is for people. Sew's policy refuses only opening
/// a new DM; K2's own `dm_not_for_agents` gate refuses every agent action on
/// such a DM either way. K2 never touches `agents.allowed_in_dms`: it is
/// install-wide and would also cut App people's agent DMs.
pub const AGENT_DM: &str = "agents.dm_with_box_people";
/// Outbox subject of the agent DM `policy_set` row.
pub const SUBJECT_AGENT_DM: &str = "policy:agents.dm_with_box_people";

/// Sew's DM calls (since 0.9.2): 1-to-1 calls between people in their DM
/// need the install policy `calls.dm_enabled` on. K2 makes sure it is on
/// while Home chat is on (prd-home-people-channels-v1 §11a.1, HP-42), through
/// the same `policy_set` row as the others. K2 never sets
/// `recording.dm_enabled` here: recording a DM call stays off unless a
/// person asks for it through the recording policy route.
pub const CALLS_DM: &str = "calls.dm_enabled";
/// Outbox subject of the DM calls `policy_set` row.
pub const SUBJECT_CALLS_DM: &str = "policy:calls.dm_enabled";

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
    if let Ok(v) = serde_json::from_str::<Value>(result) {
        if let Some(seen) = v.get("policies").and_then(|p| p.get(AGENT_DM)) {
            note_agent_dm(conn, Some(seen), now)?;
        }
        if let Some(seen) = v.get("policies").and_then(|p| p.get(CALLS_DM)) {
            note_calls_dm(conn, Some(seen), now)?;
        }
    }
    match managed_fields_in(result.as_bytes()) {
        Ok(seen) => note(conn, seen.as_ref(), now),
        Err(e) => {
            crate::log_debug!("[grant-hook] policy_set answer unreadable: {e}");
            Ok(())
        }
    }
}

/// The `agents.dm_with_box_people` value in a `{"policies": {...}}` body
/// (`None` = absent, Sew's default `true`).
pub fn agent_dm_in(body: &Value) -> Option<Value> {
    body.get("policies").unwrap_or(body).get(AGENT_DM).filter(|v| !v.is_null()).cloned()
}

/// Record what Sew showed for `agents.dm_with_box_people` (K2's value is
/// `false`; absent is Sew's default, `true`).
pub fn note_agent_dm(conn: &Connection, seen: Option<&Value>, now: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO tool_sew_policy (key, seen, is_k2, seen_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(key) DO UPDATE SET seen = excluded.seen, is_k2 = excluded.is_k2, seen_at = excluded.seen_at",
        params![AGENT_DM, seen.map(Value::to_string), (seen == Some(&Value::Bool(false))) as i64, now],
    )
    .map(|_| ())
    .map_err(|e| format!("sew policy write: {e}"))
}

/// Did Sew last show `agents.dm_with_box_people: false`? `None` = not read
/// on this install.
pub fn agent_dm_is_k2(conn: &Connection) -> Result<Option<bool>, String> {
    conn.query_row("SELECT is_k2 FROM tool_sew_policy WHERE key = ?1", params![AGENT_DM], |r| r.get::<_, i64>(0))
        .optional()
        .map(|o| o.map(|v| v != 0))
        .map_err(|e| format!("sew policy read: {e}"))
}

/// The sweep: one `policy_set {agents.dm_with_box_people: false}` when Sew
/// has the flag, K2 read the policy on this install and it isn't false yet.
pub fn plan_agent_dm(conn: &Connection, busy: &std::collections::HashSet<String>) -> Result<Vec<NewOp>, String> {
    if !features::has(conn, CHAT_KIND, features::AGENTS_DM_WITH_BOX_PEOPLE)? || busy.contains(SUBJECT_AGENT_DM) {
        return Ok(Vec::new());
    }
    if agent_dm_is_k2(conn)? != Some(false) {
        return Ok(Vec::new());
    }
    Ok(vec![NewOp {
        tool: CHAT_KIND,
        op: Op::PolicySet,
        subject_key: SUBJECT_AGENT_DM.to_string(),
        payload: json!({ "changes": { AGENT_DM: false } }),
        grant_id: None,
        grant_rev: None,
        cause: Cause::Reconcile,
        after_subject: None,
    }])
}

/// The `calls.dm_enabled` value in a `{"policies": {...}}` body (`None` =
/// absent: K2 hasn't seen Sew say it is on).
pub fn calls_dm_in(body: &Value) -> Option<Value> {
    body.get("policies").unwrap_or(body).get(CALLS_DM).filter(|v| !v.is_null()).cloned()
}

/// Record what Sew showed for `calls.dm_enabled` (K2's value is `true`).
pub fn note_calls_dm(conn: &Connection, seen: Option<&Value>, now: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO tool_sew_policy (key, seen, is_k2, seen_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(key) DO UPDATE SET seen = excluded.seen, is_k2 = excluded.is_k2, seen_at = excluded.seen_at",
        params![CALLS_DM, seen.map(Value::to_string), (seen == Some(&Value::Bool(true))) as i64, now],
    )
    .map(|_| ())
    .map_err(|e| format!("sew policy write: {e}"))
}

/// Did Sew last show `calls.dm_enabled: true`? `None` = not read on this
/// install.
pub fn calls_dm_is_k2(conn: &Connection) -> Result<Option<bool>, String> {
    conn.query_row("SELECT is_k2 FROM tool_sew_policy WHERE key = ?1", params![CALLS_DM], |r| r.get::<_, i64>(0))
        .optional()
        .map(|o| o.map(|v| v != 0))
        .map_err(|e| format!("sew policy read: {e}"))
}

/// The sweep: one `policy_set {calls.dm_enabled: true}` while Home chat is
/// on (`home_chat`), once K2 read the policy on this install and Sew doesn't
/// show it on. Nothing while Home chat is off: K2 never turns it off.
pub fn plan_calls_dm(conn: &Connection, busy: &std::collections::HashSet<String>, home_chat: bool) -> Result<Vec<NewOp>, String> {
    if !home_chat || busy.contains(SUBJECT_CALLS_DM) || calls_dm_is_k2(conn)? != Some(false) {
        return Ok(Vec::new());
    }
    Ok(vec![NewOp {
        tool: CHAT_KIND,
        op: Op::PolicySet,
        subject_key: SUBJECT_CALLS_DM.to_string(),
        payload: json!({ "changes": { CALLS_DM: true } }),
        grant_id: None,
        grant_rev: None,
        cause: Cause::Reconcile,
        after_subject: None,
    }])
}

/// A new Sew install: K2 hasn't read its policy yet.
pub fn forget(conn: &Connection) -> Result<(), String> {
    conn.execute("DELETE FROM tool_sew_policy", []).map(|_| ()).map_err(|e| format!("sew policy reset: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// HP-31: with Sew's flag, K2 sets `agents.dm_with_box_people` false once
    /// it has read the policy, and stops once Sew shows false. Without the
    /// flag nothing is sent. `agents.allowed_in_dms` is never in the body.
    #[test]
    fn agent_dm_policy_is_set_false_only_with_the_flag() {
        let c = Connection::open_in_memory().unwrap();
        ensure_schema(&c).unwrap();
        features::ensure_schema(&c).unwrap();
        let busy = std::collections::HashSet::new();
        note_agent_dm(&c, None, 1).unwrap();
        assert!(plan_agent_dm(&c, &busy).unwrap().is_empty(), "no flag: nothing");
        features::note(&c, CHAT_KIND, &[features::AGENTS_DM_WITH_BOX_PEOPLE.to_string()], 2).unwrap();
        let ops = plan_agent_dm(&c, &busy).unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].payload, json!({"changes": {"agents.dm_with_box_people": false}}));
        assert!(!ops[0].payload.to_string().contains("allowed_in_dms"));
        let busy_now: std::collections::HashSet<String> = [SUBJECT_AGENT_DM.to_string()].into();
        assert!(plan_agent_dm(&c, &busy_now).unwrap().is_empty(), "one row at a time");
        note_agent_dm(&c, agent_dm_in(&json!({"policies": {"agents.dm_with_box_people": true}})).as_ref(), 3).unwrap();
        assert_eq!(plan_agent_dm(&c, &busy).unwrap().len(), 1, "true: set again");
        note_agent_dm(&c, agent_dm_in(&json!({"policies": {"agents.dm_with_box_people": false}})).as_ref(), 4).unwrap();
        assert!(plan_agent_dm(&c, &busy).unwrap().is_empty(), "false: done");
        forget(&c).unwrap();
        assert!(plan_agent_dm(&c, &busy).unwrap().is_empty(), "a new install: read first");
    }

    /// HP-42: with Home chat on, K2 turns `calls.dm_enabled` on once it has
    /// read the policy, and stops once Sew shows it on. Home chat off: K2
    /// never sends it (and never turns it off). The body never names
    /// `recording.dm_enabled`: recording a DM call stays off by default.
    #[test]
    fn calls_dm_policy_is_turned_on_with_home_chat() {
        let c = Connection::open_in_memory().unwrap();
        ensure_schema(&c).unwrap();
        let busy = std::collections::HashSet::new();
        assert!(plan_calls_dm(&c, &busy, true).unwrap().is_empty(), "not read yet: nothing");
        note_calls_dm(&c, calls_dm_in(&json!({"policies": {}})).as_ref(), 1).unwrap();
        assert!(plan_calls_dm(&c, &busy, false).unwrap().is_empty(), "Home chat off: nothing");
        let ops = plan_calls_dm(&c, &busy, true).unwrap();
        assert_eq!(ops.len(), 1, "absent: K2 turns it on");
        assert_eq!(ops[0].payload, json!({"changes": {"calls.dm_enabled": true}}));
        assert!(!ops[0].payload.to_string().contains("recording"), "recording in DM calls is never set here");
        let busy_now: std::collections::HashSet<String> = [SUBJECT_CALLS_DM.to_string()].into();
        assert!(plan_calls_dm(&c, &busy_now, true).unwrap().is_empty(), "one row at a time");
        note_calls_dm(&c, calls_dm_in(&json!({"policies": {"calls.dm_enabled": false}})).as_ref(), 2).unwrap();
        assert_eq!(plan_calls_dm(&c, &busy, true).unwrap().len(), 1, "off: set again");
        note_calls_dm(&c, calls_dm_in(&json!({"policies": {"calls.dm_enabled": true}})).as_ref(), 3).unwrap();
        assert!(plan_calls_dm(&c, &busy, true).unwrap().is_empty(), "on: done");
    }

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
