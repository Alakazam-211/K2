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
//! Tables live in skin.db (idempotent DDL like `tool_outbox`), so this
//! needs no main-DB migration.

use std::collections::BTreeSet;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Map, Value};

use super::home;
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
         );",
    )
    .map_err(|e| format!("skin.db recording schema: {e}"))
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

/// An App was removed: forget its rollout row.
pub fn forget_app(conn: &Connection, app_id: &str) -> Result<(), String> {
    conn.execute("DELETE FROM tool_recording_rollout WHERE app_id = ?1", params![app_id])
        .map(|_| ())
        .map_err(|e| format!("recording rollout forget: {e}"))
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
    /// does, once. Inert before the pin and without `voice_record_cap`.
    #[test]
    fn rollout_sets_cap_required_only_after_the_grandfather_row() {
        let c = conn();
        home::set_app(&c, APP, false, "Jobs", "owner", 1).unwrap();
        crate::recording::set_for_tests(false, false);
        assert!(matches!(step(&c, APP, &ManifestChat::NotDeclared, true).unwrap(), Step::Skip { .. }), "before the pin");
        crate::recording::set_for_tests(true, false);
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
}
