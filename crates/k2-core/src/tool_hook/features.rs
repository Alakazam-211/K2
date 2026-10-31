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
//! | [`K2_MANAGED_AVATAR`] | K2's picture is pushed with `force` (it replaces one the member set), and K2 sets the org policy `identity.managed_fields` to `[display_name, title, pronouns, avatar]` ([`super::policy`]) |
//! | [`SPACE_UNBIND`] | an App removed from the box has its Spaces unbound in Sew too (`POST /mgmt/v1/spaces/unbind`; [`super::app_space`]) |
//! | [`HOME_CLIENT_OWNER_APPROVAL`] | every `POST /mgmt/v1/home-client` carries the install owner's approval (a fresh `k2:owner` chat pass); without the flag the body stays `{app_id}` (Sew 0.5.0 refuses an unknown field) |
//! | [`APP_CHANNEL_CREATE`] | `POST /mgmt/v1/channels/create` (Sew 0.5.0, A9): an App makes text channels in its own Space (`chat.createChannel`); without it the verb is refused `sew_feature_missing` |
//! | [`APP_GOVERNED_SPACES`] | Sew 0.6.0 (sew-55 v2.4): an App's chat key (its `max_level` and `uploads`, I7), the end of an App's sessions by `app_id` (I8), the governance switch, the role-driven channel sync, bulk channel ops, App-owned Spaces and the service owner's deprovision ([`super::governed`]). Without it each is refused `sew_feature_missing` |
//!
//! The pin (Sew 0.6.0) reports twenty flags: 0.5.0's twelve, 0.5.1's
//! `app_session_member_role`, `app_voice_channel_create`,
//! `home_client_owner_approval` and `k2_managed_guard`, and 0.6.0's
//! `app_controls`, `app_governed_spaces`, `app_member_channels` and
//! `app_mode_gate`. A flag K2 doesn't gate on is kept in the list and
//! otherwise ignored.

use rusqlite::{params, Connection, OptionalExtension};

pub const APP_SCOPED_SESSIONS: &str = "app_scoped_sessions";
pub const HOME_CLIENT: &str = "home_client";
pub const APP_ORIGIN: &str = "app_origin";
pub const K2_MANAGED_AVATAR: &str = "k2_managed_avatar";
pub const APP_CHANNEL_CREATE: &str = "app_channel_create";
pub const SPACE_UNBIND: &str = "space_unbind";
/// Sew 0.6.0 (sew-55 v2.4, `features_060`): App-governed Spaces. Gates an
/// App's chat key (I7) and the end of an App's sessions (I8), K2's
/// governance switch (I3), the role-driven channel sync (I4), bulk channel
/// ops (I6), App-owned Spaces (`spaces/create` with `owner: "app:<app_id>"`,
/// §3a) and the service owner's deprovision on uninstall
/// ([`super::governed`]). Without it each answers `sew_feature_missing`
/// and nothing is sent.
pub const APP_GOVERNED_SPACES: &str = "app_governed_spaces";
pub const HOME_CLIENT_OWNER_APPROVAL: &str = "home_client_owner_approval";

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Sew 0.6.0's `features`, exactly as `sew_proto::mgmt::FEATURES` at
    /// the v0.6.0 tag (`a8c0b652…6116`) lists them: 0.5.0's twelve, 0.5.1's
    /// four (folded into 0.6.0) and 0.6.0's own four.
    const SEW_0_6_0: [&str; 20] = [
        "app_channel_create",
        "app_controls",
        "app_governed_spaces",
        "app_member_channels",
        "app_mode_gate",
        "app_origin",
        "app_quotas",
        "app_scoped_sessions",
        "app_session_member_role",
        "app_voice_channel_create",
        "home_client",
        "home_client_owner_approval",
        "k2_managed_avatar",
        "k2_managed_guard",
        "member_external_id",
        "person_notifications",
        "principal_relink",
        "private_spaces",
        "space_unbind",
        "voice_video_cap",
    ];

    /// The pinned Sew (0.6.0) reports every flag K2 gates on: what it
    /// reports is kept as reported (sorted, nothing dropped, unknown flags
    /// included), and each gate answers yes.
    #[test]
    fn sew_0_6_0_flags_pass_every_k2_gate() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_schema(&conn).unwrap();
        let reported: Vec<String> = SEW_0_6_0.iter().rev().map(|s| s.to_string()).collect();
        assert!(note(&conn, "chat", &reported, 1).unwrap(), "a first list is a change");
        assert_eq!(list(&conn, "chat").unwrap(), SEW_0_6_0.to_vec(), "kept sorted, all twenty");
        for flag in [
            APP_SCOPED_SESSIONS,
            HOME_CLIENT,
            APP_ORIGIN,
            K2_MANAGED_AVATAR,
            APP_CHANNEL_CREATE,
            SPACE_UNBIND,
            APP_GOVERNED_SPACES,
            HOME_CLIENT_OWNER_APPROVAL,
        ] {
            assert!(has(&conn, "chat", flag).unwrap(), "{flag}");
        }
        assert!(!note(&conn, "chat", &reported, 2).unwrap(), "the same list again is no change");
        assert!(!has(&conn, "chat", "not_a_sew_flag").unwrap());
    }
}
