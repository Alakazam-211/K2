//! Sew decision 13 (Rosson 2026-10-28): Hearth as the home client. The
//! K2 half: the owner's settings and each App's signalling origin,
//! delivered to Sew over mgmt.
//!
//! Sew scopes a session by its pass's `app` claim (flag
//! `app_scoped_sessions`):
//! - an **App session** (`app` = X) sees X's bound Spaces, plus DMs whose
//!   members all share one of them;
//! - the **home client** session (`app` = the install's `home_client`)
//!   sees every Space the person is in, minus Apps flagged
//!   `hidden_from_home` (the owner's "App-only" mark); Space objects carry
//!   `app_id` and `app_display_name` for labels;
//! - no `app` claim (agents, the owner's handoff, a box login): unchanged.
//!
//! What K2 stores (skin.db `tool_sew_settings`, one row per subject: the
//! body K2 wants, and the body Sew last confirmed) and sends (outbox ops on
//! the S27 worker; Sew's shapes, confirmed 2026-10-28):
//!
//! | Subject | Op | Sew call | Body (any subset per call) |
//! |---|---|---|---|
//! | `home-client` | `home_client` | `POST /mgmt/v1/home-client` | `{app_id: "<service id>" \| null}` |
//! | `app:<id>` | `app_settings` | `POST /mgmt/v1/apps/{app_id}/settings` | `{hidden_from_home, display_name, origin}` |
//!
//! **Feature flags, never versions** ([`super::features`]): the home
//! client, `hidden_from_home` and `display_name` go only once Sew reports
//! `home_client`; `origin` (the App's `wss://` signalling origin, which
//! voice for a custom App needs) only once it reports `app_origin`. The
//! sweep plans exactly the fields Sew can take and sends again whenever
//! that subset changes (a flag appeared, the owner changed a setting, the
//! App's origin moved); a new install owes everything again.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Map, Value};

use super::{features, Cause, NewOp, Op, CHAT_KIND};

/// Subject of the install's home client.
pub const HOME_SUBJECT: &str = "home-client";
/// Prefix of an App's settings subject.
pub const APP_PREFIX: &str = "app:";
/// `POST`: the install's home client.
pub const MGMT_HOME_CLIENT: &str = "/mgmt/v1/home-client";

/// `POST /mgmt/v1/apps/{app_id}/settings`.
pub fn app_settings_path(app_id: &str) -> String {
    format!("/mgmt/v1/apps/{app_id}/settings")
}

/// Idempotent DDL, called from `skin::open_db` beside `tool_outbox`.
pub fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tool_sew_settings (
            subject    TEXT PRIMARY KEY NOT NULL,
            body       TEXT NOT NULL,
            sent_body  TEXT,
            updated_at INTEGER NOT NULL,
            updated_by TEXT NOT NULL
         );",
    )
    .map_err(|e| format!("skin.db sew settings schema: {e}"))
}

fn body_of(conn: &Connection, subject: &str) -> Result<Option<Map<String, Value>>, String> {
    let raw = conn
        .query_row("SELECT body FROM tool_sew_settings WHERE subject = ?1", params![subject], |r| r.get::<_, String>(0))
        .optional()
        .map_err(|e| format!("sew settings read: {e}"))?;
    Ok(raw.and_then(|r| serde_json::from_str::<Value>(&r).ok()).and_then(|v| v.as_object().cloned()))
}

/// Merge `fields` into `subject`'s wanted body. Returns whether it changed.
fn merge(conn: &Connection, subject: &str, fields: Map<String, Value>, actor: &str, now: i64) -> Result<bool, String> {
    let mut body = body_of(conn, subject)?.unwrap_or_default();
    let before = body.clone();
    for (k, v) in fields {
        body.insert(k, v);
    }
    if body == before && body_of(conn, subject)?.is_some() {
        return Ok(false);
    }
    conn.execute(
        "INSERT INTO tool_sew_settings (subject, body, sent_body, updated_at, updated_by) VALUES (?1, ?2, NULL, ?3, ?4)
         ON CONFLICT(subject) DO UPDATE SET body = excluded.body, updated_at = excluded.updated_at, updated_by = excluded.updated_by",
        params![subject, Value::Object(body).to_string(), now, actor],
    )
    .map_err(|e| format!("sew settings write: {e}"))?;
    Ok(true)
}

fn obj(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

/// Set (or clear, `None`) the install's home client: the App id whose
/// sessions see every Space (Hearth's service id, usually).
pub fn set_home_client(conn: &Connection, app_id: Option<&str>, actor: &str, now: i64) -> Result<bool, String> {
    merge(conn, HOME_SUBJECT, obj(json!({ "app_id": app_id })), actor, now)
}

/// Set an App's `hidden_from_home` (the owner's "App-only" mark) and the
/// name Sew labels its Spaces with.
pub fn set_app(conn: &Connection, app_id: &str, hidden: bool, display_name: &str, actor: &str, now: i64) -> Result<bool, String> {
    let subject = format!("{APP_PREFIX}{app_id}");
    merge(conn, &subject, obj(json!({ "hidden_from_home": hidden, "display_name": display_name })), actor, now)
}

/// Record an App's public signalling origin (`wss://<its host>`; `None` =
/// it has no public URL) and its label. A new App row starts not hidden.
pub fn set_app_origin(conn: &Connection, app_id: &str, origin: Option<&str>, display_name: &str, actor: &str, now: i64) -> Result<bool, String> {
    let subject = format!("{APP_PREFIX}{app_id}");
    let mut f = obj(json!({ "display_name": display_name, "origin": origin }));
    if body_of(conn, &subject)?.is_none_or(|b| !b.contains_key("hidden_from_home")) {
        f.insert("hidden_from_home".into(), Value::Bool(false));
    }
    merge(conn, &subject, f, actor, now)
}

/// The Apps with a settings row (their ids).
pub fn apps(conn: &Connection) -> Result<Vec<String>, String> {
    let mut st = conn
        .prepare("SELECT subject FROM tool_sew_settings WHERE subject LIKE 'app:%' ORDER BY subject")
        .map_err(|e| format!("sew settings list: {e}"))?;
    let rows = st
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| format!("sew settings list: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("sew settings row: {e}"))?;
    Ok(rows.into_iter().filter_map(|s| s.strip_prefix(APP_PREFIX).map(str::to_string)).collect())
}

/// Forget an App's settings (it was removed).
pub fn forget_app(conn: &Connection, app_id: &str) -> Result<(), String> {
    conn.execute("DELETE FROM tool_sew_settings WHERE subject = ?1", params![format!("{APP_PREFIX}{app_id}")])
        .map(|_| ())
        .map_err(|e| format!("sew settings delete: {e}"))
}

/// The part of `body` Sew can take now, by its flags (`None` = nothing).
pub fn sendable(subject: &str, body: &Map<String, Value>, flags: &[String]) -> Option<Value> {
    let has = |f: &str| flags.iter().any(|x| x == f);
    let mut out = Map::new();
    if subject == HOME_SUBJECT {
        if !has(features::HOME_CLIENT) {
            return None;
        }
        out.insert("app_id".into(), body.get("app_id").cloned().unwrap_or(Value::Null));
    } else {
        if has(features::HOME_CLIENT) {
            for k in ["hidden_from_home", "display_name"] {
                if let Some(v) = body.get(k) {
                    out.insert(k.into(), v.clone());
                }
            }
        }
        if has(features::APP_ORIGIN) {
            if let Some(v) = body.get("origin").filter(|v| v.is_string()) {
                out.insert("origin".into(), v.clone());
            }
            if let Some(v) = body.get("display_name") {
                out.insert("display_name".into(), v.clone());
            }
        }
        if out.is_empty() {
            return None;
        }
    }
    Some(Value::Object(out))
}

/// One stored setting (the owner's view).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Setting {
    pub subject: String,
    pub body: Value,
    /// What Sew last confirmed (`None` = nothing yet).
    pub sent: Option<Value>,
    /// Sew holds everything it can take now (by its flags).
    pub delivered: bool,
    pub updated_at: i64,
    pub updated_by: String,
}

pub fn list(conn: &Connection) -> Result<Vec<Setting>, String> {
    let flags = features::list(conn, CHAT_KIND)?;
    let mut st = conn
        .prepare("SELECT subject, body, sent_body, updated_at, updated_by FROM tool_sew_settings ORDER BY subject")
        .map_err(|e| format!("sew settings list: {e}"))?;
    let rows = st
        .query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, i64>(3)?, r.get::<_, String>(4)?))
        })
        .map_err(|e| format!("sew settings list: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("sew settings row: {e}"))?;
    Ok(rows
        .into_iter()
        .map(|(subject, body, sent, updated_at, updated_by)| {
            let body: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            let sent: Option<Value> = sent.and_then(|s| serde_json::from_str(&s).ok());
            let want = body.as_object().and_then(|b| sendable(&subject, b, &flags));
            let delivered = want.is_none_or(|w| sent.as_ref() == Some(&w));
            Setting { subject, body, sent, delivered, updated_at, updated_by }
        })
        .collect())
}

/// The sweep: one op per subject whose sendable part (by Sew's flags) isn't
/// what Sew last confirmed, skipping subjects with an open row.
pub fn plan(conn: &Connection, busy: &std::collections::HashSet<String>) -> Result<Vec<NewOp>, String> {
    let flags = features::list(conn, CHAT_KIND)?;
    let mut st = conn
        .prepare("SELECT subject, body, sent_body FROM tool_sew_settings ORDER BY subject")
        .map_err(|e| format!("sew settings plan: {e}"))?;
    let rows: Vec<(String, String, Option<String>)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map_err(|e| format!("sew settings plan: {e}"))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("sew settings plan row: {e}"))?;
    let mut out = Vec::new();
    for (subject, body, sent) in rows {
        if busy.contains(&subject) {
            continue;
        }
        let Some(body) = serde_json::from_str::<Value>(&body).ok().and_then(|v| v.as_object().cloned()) else { continue };
        let Some(want) = sendable(&subject, &body, &flags) else { continue };
        if sent.as_deref().and_then(|s| serde_json::from_str::<Value>(s).ok()).as_ref() == Some(&want) {
            continue;
        }
        let op = if subject == HOME_SUBJECT { Op::HomeClient } else { Op::AppSettings };
        out.push(NewOp {
            tool: CHAT_KIND,
            op,
            subject_key: subject,
            payload: want,
            grant_id: None,
            grant_rev: None,
            cause: Cause::Reconcile,
            after_subject: None,
        });
    }
    Ok(out)
}

/// The mgmt path a settings row goes to.
pub fn path_for(op: Op, subject: &str) -> Option<String> {
    match op {
        Op::HomeClient => Some(MGMT_HOME_CLIENT.to_string()),
        Op::AppSettings => subject.strip_prefix(APP_PREFIX).map(app_settings_path),
        _ => None,
    }
}

/// A delivered settings row: Sew holds that body now.
pub fn after_done(conn: &Connection, row_id: &str) -> Result<(), String> {
    let r: Option<(String, String, String)> = conn
        .query_row("SELECT op, subject_key, payload FROM tool_outbox WHERE id = ?1", params![row_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()
        .map_err(|e| format!("sew settings row read: {e}"))?;
    let Some((op, subject, payload)) = r else { return Ok(()) };
    if !matches!(Op::parse(&op), Some(Op::HomeClient | Op::AppSettings)) {
        return Ok(());
    }
    conn.execute("UPDATE tool_sew_settings SET sent_body = ?2 WHERE subject = ?1", params![subject, payload])
        .map(|_| ())
        .map_err(|e| format!("sew settings sent: {e}"))
}

/// A new Sew install owes every setting again.
pub fn forget_sent(conn: &Connection) -> Result<(), String> {
    conn.execute("UPDATE tool_sew_settings SET sent_body = NULL", []).map(|_| ()).map_err(|e| format!("sew settings reset: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn only_the_fields_sews_flags_allow_are_sent() {
        let app = obj(json!({"hidden_from_home": true, "display_name": "Jobs", "origin": "wss://jobs.example.com"}));
        assert_eq!(sendable("app:x", &app, &[]), None, "no flag: nothing");
        assert_eq!(sendable("app:x", &app, &f(&["home_client"])), Some(json!({"hidden_from_home": true, "display_name": "Jobs"})));
        assert_eq!(sendable("app:x", &app, &f(&["app_origin"])), Some(json!({"display_name": "Jobs", "origin": "wss://jobs.example.com"})));
        assert_eq!(
            sendable("app:x", &app, &f(&["app_origin", "home_client"])),
            Some(json!({"hidden_from_home": true, "display_name": "Jobs", "origin": "wss://jobs.example.com"}))
        );
        let no_origin = obj(json!({"hidden_from_home": false, "display_name": "Jobs", "origin": null}));
        assert_eq!(sendable("app:x", &no_origin, &f(&["app_origin"])), Some(json!({"display_name": "Jobs"})), "a null origin is not sent");
        let home = obj(json!({"app_id": null}));
        assert_eq!(sendable(HOME_SUBJECT, &home, &f(&["app_origin"])), None);
        assert_eq!(sendable(HOME_SUBJECT, &home, &f(&["home_client"])), Some(json!({"app_id": null})));
        assert_eq!(app_settings_path("abc"), "/mgmt/v1/apps/abc/settings");
        assert_eq!(path_for(Op::AppSettings, "app:abc").as_deref(), Some("/mgmt/v1/apps/abc/settings"));
        assert_eq!(path_for(Op::HomeClient, HOME_SUBJECT).as_deref(), Some(MGMT_HOME_CLIENT));
        assert_eq!(path_for(Op::Provision, "x"), None);
    }
}
