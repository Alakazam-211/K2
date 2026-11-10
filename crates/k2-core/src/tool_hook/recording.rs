//! Recording in Apps, K2's half (K2 PRD prd-k2-bobbin-in-apps-v1; Sew
//! 0.9.0). Decisions and skin.db state; the daemon does the IO.
//!
//! **Flags, never versions.** Every field below goes to Sew only once Sew
//! reports its flag ([`super::features::RecordingFeatures`]); Sew 0.8.x
//! reports none, so all of it is inert there.
//!
//! **The cap rollout (PRD §5, amendment B17).** Only with
//! [`crate::recording::in_apps`] on and Sew's `voice_record_cap`, per App
//! with a settings row (`app:<service id>`), never the home client:
//! 1. **guard:** an App whose live manifest declares chat caps but not
//!    `voice:record` gets no `cap_required` (its Spaces' passes never
//!    carried the cap, so nothing is lost; Meet waits here until its Seed
//!    lists the cap);
//! 2. **grandfather (Q1):** before `cap_required`, `voice:record` is added
//!    once to every role room line of the App that holds `voice:join`, so
//!    people who could press Record yesterday still can. One ledger row,
//!    one `deploy_audit` row and one Thread notice per App, actor
//!    [`ACTOR_ROLLOUT`]. It never touches `start_level`, so a CUI/FCI-locked
//!    Space stays at `none`. Owners narrow it afterwards;
//! 3. **`cap_required`:** only once the App's grandfather row exists, the
//!    App's settings row gains `recording.cap_required: true`, which the
//!    sweep sends with `voice_record_cap`.
//!
//! **Settings (K-c).** An App's `recording` object lives in its
//! `tool_sew_settings` row ([`set_app_recording`]); a Space's overrides
//! (`recording_start_level`, `recording_required`; `null` inherits) live in
//! its `space:<id>` row beside `governance` ([`set_space_recording`]), and go
//! with the flags `app_recording` / `recording_required` (B7), with no
//! governance change. The K5 rooms' overrides use the K5 Space's row the same
//! way (no `app_id`).
//!
//! **The CUI/FCI lock** (`app_recording_locks`, Q7: the owner only, in
//! person). A locked Space's row always sends `recording_start_level: none`
//! and `recording_required: false`, whatever the App or anyone asks
//! ([`locked_body`]); the lock survives an unbind on purpose (B7).
//!
//! **Org policies** (`recording.enabled`, `.dm_enabled`,
//! `.require_consent_to_join`, `.required`, `.audio_max_days`): K2 keeps what
//! a person asked for (`tool_recording_policy`), reads what Sew shows each
//! sweep, and queues one `policy_set` with the keys that differ, like
//! `identity.managed_fields` ([`super::policy`]).
//!
//! Tables live in skin.db (idempotent DDL like `tool_outbox`), so this
//! needs no main-DB migration.

use std::collections::{BTreeSet, HashSet};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Map, Value};

use super::sew_wire::recording as wire;
use super::{features, governed, home, Cause, NewOp, Op, CHAT_KIND};
use crate::skin::{RoomPolicy, CAP_VOICE_JOIN, CAP_VOICE_RECORD};

/// The actor on every row the rollout writes (attribution only).
pub const ACTOR_ROLLOUT: &str = "k2:recording-rollout";
/// The `deploy_audit` event of the grandfather step.
pub const AUDIT_GRANDFATHER: &str = "recording_cap_grandfathered";

/// Idempotent DDL, called from `skin::open_db` beside `tool_outbox`.
pub fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tool_recording_rollout (
            app_id            TEXT PRIMARY KEY NOT NULL,
            grandfathered_at  INTEGER,
            grandfather_roles TEXT NOT NULL DEFAULT '[]',
            cap_required_at   INTEGER
         );
         CREATE TABLE IF NOT EXISTS app_recording_locks (
            space_id  TEXT PRIMARY KEY NOT NULL,
            locked_by TEXT NOT NULL,
            locked_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS tool_recording_feed (
            app_id  TEXT PRIMARY KEY NOT NULL,
            cursor  TEXT,
            seen_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS tool_recording_policy (
            key     TEXT PRIMARY KEY NOT NULL,
            want    TEXT,
            set_by  TEXT,
            set_at  INTEGER,
            seen    TEXT,
            seen_at INTEGER
         );",
    )
    .map_err(|e| format!("skin.db recording schema: {e}"))
}

// ── Space overrides and the CUI/FCI lock ─────────────────────────────

/// Merge a Space's recording overrides into its `space:<id>` row (`Some(Value::Null)`
/// inherits; `None` keeps). Returns whether it changed.
pub fn set_space_recording(
    conn: &Connection,
    space_id: &str,
    start_level: Option<Value>,
    required: Option<Value>,
    actor: &str,
    now: i64,
) -> Result<bool, String> {
    let mut f = Map::new();
    if let Some(v) = start_level {
        f.insert("recording_start_level".into(), v);
    }
    if let Some(v) = required {
        f.insert("recording_required".into(), v);
    }
    governed::merge_space_settings(conn, space_id, f, actor, now)
}

/// A Space's overrides as K2 wants them: `(start_level, required)`, each
/// `None` when not set (or set to inherit).
pub fn space_recording(conn: &Connection, space_id: &str) -> Result<(Option<String>, Option<bool>), String> {
    let b = governed::space_body(conn, space_id)?.unwrap_or_default();
    Ok((
        b.get("recording_start_level").and_then(Value::as_str).map(str::to_string),
        b.get("recording_required").and_then(Value::as_bool),
    ))
}

/// One CUI/FCI lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Lock {
    pub space_id: String,
    pub locked_by: String,
    pub locked_at: i64,
}

/// Is `space_id` CUI/FCI-locked (no recording, whatever anyone asks)?
pub fn is_locked(conn: &Connection, space_id: &str) -> Result<bool, String> {
    conn.query_row("SELECT 1 FROM app_recording_locks WHERE space_id = ?1", params![space_id], |_| Ok(()))
        .optional()
        .map(|o| o.is_some())
        .map_err(|e| format!("recording lock read: {e}"))
}

/// Every lock, by Space.
pub fn locks(conn: &Connection) -> Result<Vec<Lock>, String> {
    let mut st = conn
        .prepare("SELECT space_id, locked_by, locked_at FROM app_recording_locks ORDER BY space_id")
        .map_err(|e| format!("recording locks: {e}"))?;
    let rows = st
        .query_map([], |r| Ok(Lock { space_id: r.get(0)?, locked_by: r.get(1)?, locked_at: r.get(2)? }))
        .map_err(|e| format!("recording locks: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("recording lock row: {e}"))?;
    Ok(rows)
}

/// Set or lift a Space's lock. Locking also writes the no-recording
/// overrides into the Space's row (so a Space without a row gets one);
/// lifting sets them back to inherit. The caller checked the owner in
/// person. Returns whether it changed.
pub fn set_lock(conn: &Connection, space_id: &str, on: bool, by: &str, now: i64) -> Result<bool, String> {
    let was = is_locked(conn, space_id)?;
    if on == was {
        return Ok(false);
    }
    if on {
        conn.execute(
            "INSERT INTO app_recording_locks (space_id, locked_by, locked_at) VALUES (?1, ?2, ?3)",
            params![space_id, by, now],
        )
        .map_err(|e| format!("recording lock write: {e}"))?;
        set_space_recording(conn, space_id, Some(json!(wire::START_NONE)), Some(json!(false)), by, now)?;
    } else {
        conn.execute("DELETE FROM app_recording_locks WHERE space_id = ?1", params![space_id])
            .map_err(|e| format!("recording lock write: {e}"))?;
        set_space_recording(conn, space_id, Some(Value::Null), Some(Value::Null), by, now)?;
    }
    Ok(true)
}

/// Every locked Space has a row saying no recording (an unbind deletes a
/// Space's row; the lock survives it, B7). Returns the rows written.
pub fn ensure_lock_rows(conn: &Connection, now: i64) -> Result<usize, String> {
    let mut n = 0;
    for l in locks(conn)? {
        if governed::space_body(conn, &l.space_id)?.is_none() {
            set_space_recording(conn, &l.space_id, Some(json!(wire::START_NONE)), Some(json!(false)), &l.locked_by, now)?;
            n += 1;
        }
    }
    Ok(n)
}

/// A delivered recording `policy_set`: what Sew's answer shows.
pub fn after_policy_done(conn: &Connection, row_id: &str, result: &str, now: i64) -> Result<(), String> {
    let subject: Option<String> = conn
        .query_row("SELECT subject_key FROM tool_outbox WHERE id = ?1", params![row_id], |r| r.get(0))
        .optional()
        .map_err(|e| format!("recording policy row read: {e}"))?;
    if subject.as_deref() != Some(POLICY_SUBJECT) {
        return Ok(());
    }
    match serde_json::from_str::<Value>(result) {
        Ok(v) => note_seen_policies(conn, &v, now),
        Err(_) => Ok(()),
    }
}

/// A Space row's body as the sweep sends it: locked, never more than no
/// recording. Not locked: unchanged.
pub fn locked_body(conn: &Connection, space_id: &str, mut body: Map<String, Value>) -> Result<Map<String, Value>, String> {
    if is_locked(conn, space_id)? {
        body.insert("recording_start_level".into(), json!(wire::START_NONE));
        body.insert("recording_required".into(), json!(false));
    }
    Ok(body)
}

// ── org policies ─────────────────────────────────────────────────────

/// Outbox subject of the recording `policy_set` row.
pub const POLICY_SUBJECT: &str = "policy:recording";

/// Is `key` = `value` a recording policy Sew takes?
pub fn policy_value_ok(key: &str, value: &Value) -> Result<(), String> {
    match key {
        wire::POLICY_ENABLED | wire::POLICY_DM_ENABLED | wire::POLICY_REQUIRE_CONSENT_TO_JOIN => {
            value.is_boolean().then_some(()).ok_or_else(|| format!("{key} is true or false"))
        }
        wire::POLICY_REQUIRED => matches!(value.as_str(), Some(wire::REQUIRED_OFF | wire::REQUIRED_MULTI_PERSON))
            .then_some(())
            .ok_or_else(|| format!("{key} is off or multi_person")),
        wire::POLICY_AUDIO_MAX_DAYS => value
            .as_u64()
            .filter(|d| (1..=3650).contains(d))
            .map(|_| ())
            .ok_or_else(|| format!("{key} is a number of days, 1 to 3650")),
        _ => Err(format!("{key} isn't a recording policy ({})", wire::POLICY_KEYS.join(", "))),
    }
}

/// The flag a policy key needs.
pub fn policy_flag(key: &str) -> &'static str {
    if key == wire::POLICY_REQUIRED {
        features::RECORDING_REQUIRED
    } else {
        features::APP_RECORDING
    }
}

/// Ask for a recording policy (checked; the caller checked the person).
pub fn set_policy(conn: &Connection, key: &str, value: &Value, by: &str, now: i64) -> Result<bool, String> {
    policy_value_ok(key, value)?;
    let raw = value.to_string();
    let before: Option<Option<String>> = conn
        .query_row("SELECT want FROM tool_recording_policy WHERE key = ?1", params![key], |r| r.get(0))
        .optional()
        .map_err(|e| format!("recording policy read: {e}"))?;
    if before.flatten().as_deref() == Some(raw.as_str()) {
        return Ok(false);
    }
    conn.execute(
        "INSERT INTO tool_recording_policy (key, want, set_by, set_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(key) DO UPDATE SET want = excluded.want, set_by = excluded.set_by, set_at = excluded.set_at",
        params![key, raw, by, now],
    )
    .map_err(|e| format!("recording policy write: {e}"))?;
    Ok(true)
}

/// One recording policy: what K2 was asked for and what Sew last showed.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyRow {
    pub key: String,
    pub want: Option<Value>,
    pub seen: Option<Value>,
    pub set_by: Option<String>,
    pub delivered: bool,
}

pub fn policies(conn: &Connection) -> Result<Vec<PolicyRow>, String> {
    let mut out = Vec::new();
    for key in wire::POLICY_KEYS {
        let r: Option<(Option<String>, Option<String>, Option<String>)> = conn
            .query_row("SELECT want, seen, set_by FROM tool_recording_policy WHERE key = ?1", params![key], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()
            .map_err(|e| format!("recording policy read: {e}"))?;
        let (want, seen, set_by) = r.unwrap_or_default();
        let want: Option<Value> = want.and_then(|w| serde_json::from_str(&w).ok());
        let seen: Option<Value> = seen.and_then(|s| serde_json::from_str(&s).ok());
        let delivered = want.as_ref().is_none_or(|w| seen.as_ref() == Some(w));
        out.push(PolicyRow { key: key.to_string(), want, seen, set_by, delivered });
    }
    Ok(out)
}

/// Record what Sew showed (`{"policies": {...}}` or its inner object).
pub fn note_seen_policies(conn: &Connection, body: &Value, now: i64) -> Result<(), String> {
    let p = body.get("policies").unwrap_or(body);
    let Some(p) = p.as_object() else { return Ok(()) };
    for key in wire::POLICY_KEYS {
        if let Some(v) = p.get(key) {
            conn.execute(
                "INSERT INTO tool_recording_policy (key, seen, seen_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET seen = excluded.seen, seen_at = excluded.seen_at",
                params![key, v.to_string(), now],
            )
            .map_err(|e| format!("recording policy seen: {e}"))?;
        }
    }
    Ok(())
}

/// The org recording policies' value, as the org's `audio_max_days` K2
/// last saw (B23's ceiling for an App's keep days).
pub fn org_audio_max_days(conn: &Connection) -> Result<Option<u64>, String> {
    Ok(policies(conn)?.into_iter().find(|p| p.key == wire::POLICY_AUDIO_MAX_DAYS).and_then(|p| p.seen.or(p.want)).and_then(|v| v.as_u64()))
}

/// The sweep: one `policy_set` with every wanted key Sew can take (by its
/// flags) and doesn't show yet. Nothing while a row is open.
pub fn plan_policies(conn: &Connection, busy: &HashSet<String>) -> Result<Vec<NewOp>, String> {
    if busy.contains(POLICY_SUBJECT) {
        return Ok(Vec::new());
    }
    let flags = features::list(conn, CHAT_KIND)?;
    let mut changes = Map::new();
    for p in policies(conn)? {
        let Some(want) = p.want else { continue };
        if !flags.iter().any(|f| f == policy_flag(&p.key)) || p.seen.as_ref() == Some(&want) {
            continue;
        }
        changes.insert(p.key, want);
    }
    if changes.is_empty() {
        return Ok(Vec::new());
    }
    Ok(vec![NewOp {
        tool: CHAT_KIND,
        op: Op::PolicySet,
        subject_key: POLICY_SUBJECT.to_string(),
        payload: json!({ "changes": changes }),
        grant_id: None,
        grant_rev: None,
        cause: Cause::Reconcile,
        after_subject: None,
    }])
}

/// A new Sew install: K2 hasn't seen its policies yet (wishes stay).
pub fn forget_seen_policies(conn: &Connection) -> Result<(), String> {
    conn.execute("UPDATE tool_recording_policy SET seen = NULL, seen_at = NULL", [])
        .map(|_| ())
        .map_err(|e| format!("recording policy reset: {e}"))
}

// ── the App's recording object (`tool_sew_settings` `app:<id>`) ──────

/// The App's wanted `recording` object (empty = nothing set).
pub fn app_recording(conn: &Connection, app_id: &str) -> Result<Map<String, Value>, String> {
    Ok(home::app_body(conn, app_id)?
        .and_then(|b| b.get("recording").and_then(Value::as_object).cloned())
        .unwrap_or_default())
}

/// Merge `fields` into the App's `recording` object (`null` clears a
/// field). Returns whether it changed.
pub fn set_app_recording(conn: &Connection, app_id: &str, fields: &Map<String, Value>, actor: &str, now: i64) -> Result<bool, String> {
    let mut rec = app_recording(conn, app_id)?;
    for (k, v) in fields {
        if v.is_null() {
            rec.remove(k);
        } else {
            rec.insert(k.clone(), v.clone());
        }
    }
    home::merge_app_field(conn, app_id, "recording", Value::Object(rec), actor, now)
}

// ── the cap rollout ──────────────────────────────────────────────────

/// What the App's live manifest says about chat caps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestChat {
    /// No live manifest, or one that lists no chat cap (Sew-governed App
    /// Spaces: today's roles decide).
    NotDeclared,
    /// Its `caps` (the chat ones).
    Declared(Vec<String>),
}

/// One App's next rollout step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum Step {
    /// Nothing to do now, and why.
    Skip { why: String },
    /// Add `voice:record` to the App's `voice:join` role lines first.
    Grandfather,
    /// Set `recording.cap_required: true`.
    CapRequired,
    /// `cap_required` is set.
    Done,
}

fn skip(why: &str) -> Step {
    Step::Skip { why: why.to_string() }
}

/// The rollout's next step for `app_id`. Pure over skin.db and `manifest`;
/// `record_cap` = Sew reports `voice_record_cap`.
pub fn step(conn: &Connection, app_id: &str, manifest: &ManifestChat, record_cap: bool) -> Result<Step, String> {
    if !crate::recording::in_apps() {
        return Ok(skip("voice:record waits for K2's Sew 0.9.0 release"));
    }
    if !record_cap {
        return Ok(skip("this box's Sew doesn't report voice_record_cap"));
    }
    if home::is_home_client(conn, app_id)? {
        return Ok(skip("the home client takes no recording settings"));
    }
    if app_recording(conn, app_id)?.get("cap_required") == Some(&Value::Bool(true)) {
        return Ok(Step::Done);
    }
    if let ManifestChat::Declared(caps) = manifest {
        if !caps.iter().any(|c| c == CAP_VOICE_RECORD) {
            return Ok(skip("its manifest asks for chat caps but not voice:record"));
        }
    }
    if grandfathered_at(conn, app_id)?.is_none() {
        return Ok(Step::Grandfather);
    }
    Ok(Step::CapRequired)
}

/// `policy` with `voice:record` added to every room line that holds
/// `voice:join` and lacks it; `None` = nothing to add.
pub fn grandfather_policy(policy: &RoomPolicy) -> Option<RoomPolicy> {
    let mut out = policy.clone();
    let mut changed = false;
    for caps in out.values_mut() {
        if caps.iter().any(|c| c == CAP_VOICE_JOIN) && !caps.iter().any(|c| c == CAP_VOICE_RECORD) {
            caps.push(CAP_VOICE_RECORD.to_string());
            changed = true;
        }
    }
    changed.then_some(out)
}

/// When the App's grandfather step ran (`None` = never).
pub fn grandfathered_at(conn: &Connection, app_id: &str) -> Result<Option<i64>, String> {
    conn.query_row("SELECT grandfathered_at FROM tool_recording_rollout WHERE app_id = ?1", params![app_id], |r| r.get::<_, Option<i64>>(0))
        .optional()
        .map(Option::flatten)
        .map_err(|e| format!("recording rollout read: {e}"))
}

/// Record the App's grandfather step (the roles it widened, by name).
pub fn note_grandfathered(conn: &Connection, app_id: &str, roles: &[String], now: i64) -> Result<(), String> {
    let roles: BTreeSet<&String> = roles.iter().collect();
    conn.execute(
        "INSERT INTO tool_recording_rollout (app_id, grandfathered_at, grandfather_roles) VALUES (?1, ?2, ?3)
         ON CONFLICT(app_id) DO UPDATE SET grandfathered_at = excluded.grandfathered_at, grandfather_roles = excluded.grandfather_roles",
        params![app_id, now, json!(roles).to_string()],
    )
    .map(|_| ())
    .map_err(|e| format!("recording rollout write: {e}"))
}

/// Set the App's `cap_required` (only after its grandfather row).
pub fn set_cap_required(conn: &Connection, app_id: &str, now: i64) -> Result<bool, String> {
    if grandfathered_at(conn, app_id)?.is_none() {
        return Err(format!("App {app_id}: cap_required comes only after its grandfather step"));
    }
    let mut f = Map::new();
    f.insert("cap_required".into(), Value::Bool(true));
    let changed = set_app_recording(conn, app_id, &f, ACTOR_ROLLOUT, now)?;
    conn.execute("UPDATE tool_recording_rollout SET cap_required_at = ?2 WHERE app_id = ?1", params![app_id, now])
        .map_err(|e| format!("recording rollout write: {e}"))?;
    Ok(changed)
}

/// An App was removed: forget its rollout row and its feed cursor (B11).
pub fn forget_app(conn: &Connection, app_id: &str) -> Result<(), String> {
    conn.execute("DELETE FROM tool_recording_rollout WHERE app_id = ?1", params![app_id])
        .map_err(|e| format!("recording rollout forget: {e}"))?;
    conn.execute("DELETE FROM tool_recording_feed WHERE app_id = ?1", params![app_id])
        .map(|_| ())
        .map_err(|e| format!("recording feed forget: {e}"))
}

// ── the App feed (K-d) ───────────────────────────────────────────────

/// The App's feed cursor (`None` = never read, or Sew dropped it).
pub fn feed_cursor(conn: &Connection, app_id: &str) -> Result<Option<String>, String> {
    conn.query_row("SELECT cursor FROM tool_recording_feed WHERE app_id = ?1", params![app_id], |r| r.get::<_, Option<String>>(0))
        .optional()
        .map(Option::flatten)
        .map_err(|e| format!("recording feed read: {e}"))
}

/// Keep the App's feed cursor (`None` clears it: start again).
pub fn set_feed_cursor(conn: &Connection, app_id: &str, cursor: Option<&str>, now: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO tool_recording_feed (app_id, cursor, seen_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(app_id) DO UPDATE SET cursor = excluded.cursor, seen_at = excluded.seen_at",
        params![app_id, cursor, now],
    )
    .map(|_| ())
    .map_err(|e| format!("recording feed write: {e}"))
}

/// Why K2 refuses an App server's read before asking Sew.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadRefusal {
    /// Sew doesn't report `mgmt_recording_reads`.
    FeatureMissing,
    /// The App's `server_read` is off (the owner turns it on, on a card).
    ServerReadOff,
    /// The App governs no Space (or not this one): a participant App.
    NotGoverning,
}

/// K2's own checks of an App server's recording read (PRD K-d): Sew's
/// flag, the App's `server_read` (as K2 wants it: on only through the
/// owner's card), and, for one Space, that the App governs it (it is
/// bound to it); with no Space, that it governs at least one.
pub fn read_gate(conn: &Connection, app_id: &str, space: Option<&str>) -> Result<Result<(), ReadRefusal>, String> {
    if !features::has(conn, CHAT_KIND, features::MGMT_RECORDING_READS)? {
        return Ok(Err(ReadRefusal::FeatureMissing));
    }
    if app_recording(conn, app_id)?.get("server_read") != Some(&Value::Bool(true)) || home::is_home_client(conn, app_id)? {
        return Ok(Err(ReadRefusal::ServerReadOff));
    }
    let governs = match space {
        Some(s) => super::app_space::bound_app(conn, s)?.as_deref() == Some(app_id),
        None => super::app_space::bindings(conn)?.iter().any(|b| b.app_id == app_id),
    };
    if !governs {
        return Ok(Err(ReadRefusal::NotGoverning));
    }
    Ok(Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        home::ensure_schema(&c).unwrap();
        super::super::features::ensure_schema(&c).unwrap();
        ensure_schema(&c).unwrap();
        c
    }

    const APP: &str = "7d4e2a10-1111-4222-8333-444455556666";

    /// PRD §8 K-b "rollout": an App with no role holding `voice:record`
    /// gets no `cap_required` until its grandfather row exists; then it
    /// does, once. Inert without `voice_record_cap` (a box still on Sew 0.8.x;
    /// the pin's const is on since K2 0.46.11).
    #[test]
    fn rollout_sets_cap_required_only_after_the_grandfather_row() {
        let c = conn();
        home::set_app(&c, APP, false, "Jobs", "owner", 1).unwrap();
        crate::recording::set_for_tests(false, false);
        assert!(crate::recording::RECORDING_IN_APPS, "the pin's const is on");
        assert!(matches!(step(&c, APP, &ManifestChat::NotDeclared, false).unwrap(), Step::Skip { .. }), "Sew 0.8.x");
        assert_eq!(step(&c, APP, &ManifestChat::NotDeclared, true).unwrap(), Step::Grandfather);
        let e = set_cap_required(&c, APP, 2).unwrap_err();
        assert!(e.contains("after its grandfather step"), "{e}");
        assert_eq!(app_recording(&c, APP).unwrap().get("cap_required"), None);
        note_grandfathered(&c, APP, &["crew".into()], 3).unwrap();
        assert_eq!(step(&c, APP, &ManifestChat::NotDeclared, true).unwrap(), Step::CapRequired);
        assert!(set_cap_required(&c, APP, 4).unwrap());
        assert_eq!(step(&c, APP, &ManifestChat::NotDeclared, true).unwrap(), Step::Done);
        assert!(!set_cap_required(&c, APP, 5).unwrap(), "once");
        crate::recording::set_for_tests(false, false);
    }

    /// §5 step 3: an App that declares chat caps but not `voice:record`
    /// (Meet before its Seed lists the cap) is never capped; the home
    /// client never takes recording settings (B22).
    #[test]
    fn the_guard_and_the_home_client_are_skipped() {
        let c = conn();
        crate::recording::set_for_tests(true, false);
        home::set_app(&c, APP, false, "Meet", "owner", 1).unwrap();
        let meet = ManifestChat::Declared(vec!["chat:read".into(), "voice:join".into()]);
        assert!(matches!(step(&c, APP, &meet, true).unwrap(), Step::Skip { why } if why.contains("not voice:record")));
        let meet = ManifestChat::Declared(vec!["chat:read".into(), "voice:join".into(), "voice:record".into()]);
        assert_eq!(step(&c, APP, &meet, true).unwrap(), Step::Grandfather);
        home::set_home_client(&c, Some(APP), "owner", 2).unwrap();
        assert!(matches!(step(&c, APP, &meet, true).unwrap(), Step::Skip { why } if why.contains("home client")));
        crate::recording::set_for_tests(false, false);
    }

    #[test]
    fn grandfather_adds_record_to_voice_lines_only() {
        let mut p = RoomPolicy::new();
        p.insert("a".into(), vec!["thread:read".into(), "voice:join".into()]);
        p.insert("b".into(), vec!["chat:read".into()]);
        p.insert("c".into(), vec!["voice:join".into(), "voice:record".into()]);
        let g = grandfather_policy(&p).expect("a line to widen");
        assert_eq!(g["a"], vec!["thread:read", "voice:join", "voice:record"]);
        assert_eq!(g["b"], p["b"]);
        assert_eq!(g["c"], p["c"]);
        assert_eq!(grandfather_policy(&g), None, "once");
    }

    fn flags(f: &[&str]) -> Vec<String> {
        f.iter().map(|s| s.to_string()).collect()
    }

    /// PRD §8 K-c, B7: a Space's recording override goes with no governance
    /// change, each field only with its flag, and the switch keeps the
    /// override (it merges).
    #[test]
    fn a_space_override_goes_without_a_governance_change() {
        let c = conn();
        governed::ensure_schema(&c).unwrap();
        set_space_recording(&c, "5101", Some(json!("member")), Some(json!(true)), "owner", 1).unwrap();
        let body = governed::space_body(&c, "5101").unwrap().expect("a row");
        assert_eq!(governed::sendable(&body, &flags(&["app_governed_spaces"])), None, "Sew 0.8.x: nothing to send");
        assert_eq!(
            governed::sendable(&body, &flags(&["app_recording"])),
            Some(json!({"recording_start_level": "member"})),
            "required waits for its own flag"
        );
        assert_eq!(
            governed::sendable(&body, &flags(&["app_recording", "recording_required"])),
            Some(json!({"recording_start_level": "member", "recording_required": true}))
        );
        governed::set_governance(&c, "5101", "app", "owner", 2).unwrap();
        let body = governed::space_body(&c, "5101").unwrap().unwrap();
        assert_eq!(
            governed::sendable(&body, &flags(&["app_governed_spaces", "app_recording", "recording_required"])),
            Some(json!({"governance": "app", "recording_start_level": "member", "recording_required": true})),
            "the switch kept the override"
        );
        set_space_recording(&c, "5101", Some(Value::Null), None, "owner", 3).unwrap();
        let body = governed::space_body(&c, "5101").unwrap().unwrap();
        assert_eq!(governed::sendable(&body, &flags(&["app_recording"])), Some(json!({"recording_start_level": null})), "null inherits, sent as null");
    }

    /// PRD §8 K-c, Q7: a locked Space sends `start_level: none` and
    /// `required: false` whatever its row asks; lifting sends inherit; the
    /// lock survives an unbind (its row comes back).
    #[test]
    fn a_locked_space_never_records() {
        let c = conn();
        governed::ensure_schema(&c).unwrap();
        let all = flags(&["app_recording", "recording_required"]);
        set_space_recording(&c, "5101", Some(json!("member")), Some(json!(true)), "agent:jobs", 1).unwrap();
        assert!(set_lock(&c, "5101", true, "owner", 2).unwrap());
        assert!(!set_lock(&c, "5101", true, "owner", 3).unwrap(), "once");
        // Someone asks again for recording: the lock wins at send time.
        set_space_recording(&c, "5101", Some(json!("member")), Some(json!(true)), "agent:jobs", 4).unwrap();
        let sent = home::sendable_now(&c, "space:5101", &governed::space_body(&c, "5101").unwrap().unwrap(), &all).unwrap();
        assert_eq!(sent, Some(json!({"recording_start_level": "none", "recording_required": false})));
        // An unbind forgets the row; the lock puts it back.
        governed::forget_space(&c, "5101").unwrap();
        assert_eq!(governed::space_body(&c, "5101").unwrap(), None);
        assert_eq!(ensure_lock_rows(&c, 5).unwrap(), 1);
        let sent = home::sendable_now(&c, "space:5101", &governed::space_body(&c, "5101").unwrap().unwrap(), &all).unwrap();
        assert_eq!(sent, Some(json!({"recording_start_level": "none", "recording_required": false})));
        assert!(set_lock(&c, "5101", false, "owner", 6).unwrap());
        let sent = home::sendable_now(&c, "space:5101", &governed::space_body(&c, "5101").unwrap().unwrap(), &all).unwrap();
        assert_eq!(sent, Some(json!({"recording_start_level": null, "recording_required": null})), "lifted: inherit");
        assert!(locks(&c).unwrap().is_empty());
    }

    /// Org policies: checked values, one `policy_set` with what differs and
    /// what Sew's flags allow, nothing once Sew shows it.
    #[test]
    fn org_policies_reconcile_like_managed_fields() {
        let c = conn();
        super::super::outbox::ensure_schema(&c).unwrap();
        assert!(set_policy(&c, wire::POLICY_REQUIRED, &json!("always"), "owner", 1).is_err());
        assert!(set_policy(&c, wire::POLICY_AUDIO_MAX_DAYS, &json!(0), "owner", 1).is_err());
        assert!(set_policy(&c, "recording.bogus", &json!(true), "owner", 1).is_err());
        set_policy(&c, wire::POLICY_REQUIRED, &json!("multi_person"), "owner", 1).unwrap();
        set_policy(&c, wire::POLICY_ENABLED, &json!(true), "owner", 1).unwrap();
        let busy = HashSet::new();
        assert!(plan_policies(&c, &busy).unwrap().is_empty(), "Sew 0.8.x: nothing");
        features::note(&c, CHAT_KIND, &flags(&["app_recording"]), 2).unwrap();
        let ops = plan_policies(&c, &busy).unwrap();
        assert_eq!(ops[0].payload, json!({"changes": {"recording.enabled": true}}), "recording.required waits for its flag");
        features::note(&c, CHAT_KIND, &flags(&["app_recording", "recording_required"]), 3).unwrap();
        let ops = plan_policies(&c, &busy).unwrap();
        assert_eq!(ops[0].payload, json!({"changes": {"recording.enabled": true, "recording.required": "multi_person"}}));
        assert_eq!(ops[0].subject_key, POLICY_SUBJECT);
        note_seen_policies(&c, &json!({"policies": {"recording.enabled": true, "recording.required": "multi_person", "recording.audio_max_days": 30}}), 4).unwrap();
        assert!(plan_policies(&c, &busy).unwrap().is_empty(), "Sew shows it: nothing more");
        assert_eq!(org_audio_max_days(&c).unwrap(), Some(30));
        assert!(policies(&c).unwrap().iter().all(|p| p.delivered));
    }
}
