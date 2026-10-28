//! What the box's Sew can do, by feature flag (Sew 2026-10-28): `GET
//! /mgmt/v1/status` (and `sew version --json`) carries `features: [...]`,
//! a sorted list whose names never change. **K2 gates on these flags,
//! never on Sew version numbers.** The reconcile sweep records the list it
//! saw ([`note`]); readers ask [`has`].
//!
//! | Flag | What K2 does with it |
//! |---|---|
//! | [`APP_SCOPED_SESSIONS`] | Sew scopes a session by its pass's `app` claim. Until it shows, an App may not give its users chat (`guests.setChat` is refused) and the owner is told the App sees all chat its person can |
//! | [`HOME_CLIENT`] | the install's home client and an App's `hidden_from_home` ("App-only") are sent |
//! | [`APP_ORIGIN`] | an App's `wss://` signalling origin is sent (voice for custom Apps needs it) |
//! | [`K2_MANAGED_AVATAR`] | K2's picture is pushed with `force` (it replaces one the member set) |
//! | [`APP_CHANNEL_CREATE`] | `POST /mgmt/v1/channels/create` (Sew 0.5.0, A9): an App makes text channels in its own Space (`chat.createChannel`); without it the verb is refused `sew_feature_missing` |

use rusqlite::{params, Connection, OptionalExtension};

pub const APP_SCOPED_SESSIONS: &str = "app_scoped_sessions";
pub const HOME_CLIENT: &str = "home_client";
pub const APP_ORIGIN: &str = "app_origin";
pub const K2_MANAGED_AVATAR: &str = "k2_managed_avatar";
pub const APP_CHANNEL_CREATE: &str = "app_channel_create";

/// Idempotent DDL, called from `skin::open_db` beside `tool_outbox`.
pub fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tool_target_features (
            kind     TEXT PRIMARY KEY NOT NULL,
            features TEXT NOT NULL,
            seen_at  INTEGER NOT NULL
         );",
    )
    .map_err(|e| format!("skin.db target features schema: {e}"))
}

/// Record the flags Sew reported for `kind` (sorted, de-duplicated).
/// Returns whether they changed.
pub fn note(conn: &Connection, kind: &str, features: &[String], now: i64) -> Result<bool, String> {
    let mut f: Vec<String> = features.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    f.sort();
    f.dedup();
    let raw = serde_json::to_string(&f).map_err(|e| e.to_string())?;
    let before = conn
        .query_row("SELECT features FROM tool_target_features WHERE kind = ?1", params![kind], |r| r.get::<_, String>(0))
        .optional()
        .map_err(|e| format!("target features read: {e}"))?;
    conn.execute(
        "INSERT INTO tool_target_features (kind, features, seen_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(kind) DO UPDATE SET features = excluded.features, seen_at = excluded.seen_at",
        params![kind, raw, now],
    )
    .map_err(|e| format!("target features write: {e}"))?;
    Ok(before.as_deref() != Some(raw.as_str()))
}

/// The flags Sew last reported for `kind` (empty = none seen yet).
pub fn list(conn: &Connection, kind: &str) -> Result<Vec<String>, String> {
    let raw = conn
        .query_row("SELECT features FROM tool_target_features WHERE kind = ?1", params![kind], |r| r.get::<_, String>(0))
        .optional()
        .map_err(|e| format!("target features read: {e}"))?;
    Ok(raw.and_then(|r| serde_json::from_str(&r).ok()).unwrap_or_default())
}

/// Did Sew report `flag` for `kind`?
pub fn has(conn: &Connection, kind: &str, flag: &str) -> Result<bool, String> {
    Ok(list(conn, kind)?.iter().any(|f| f == flag))
}
