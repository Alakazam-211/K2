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
//! | [`SERVER_GOVERNED_SPACE`], [`MGMT_VOICE_STATES`], [`VOICE_CHANNEL_CLOSING`], [`TURN_RELAY_ONLY`], [`VOICE_MEDIA_PATH`], [`IMPORT_IDENTITY_MAP`] | Sew 0.7.0 (K2 voice rooms, sew-56): read together as [`VoiceFeatures`] (`tool_hook::sew_voice_features()`), one read of the list the last status refresh recorded |
//! | [`APP_GOVERNED_SPACES`] | Sew 0.6.0 (sew-55 v2.4): an App's chat key (its `max_level` and `uploads`, I7), the end of an App's sessions by `app_id` (I8), the governance switch, the role-driven channel sync, bulk channel ops, App-owned Spaces and the service owner's deprovision ([`super::governed`]). Without it each is refused `sew_feature_missing` |
//!
//! The pin (Sew 0.6.1; the same twenty flags as 0.6.0) reports 0.5.0's
//! twelve, 0.5.1's `app_session_member_role`, `app_voice_channel_create`,
//! `home_client_owner_approval` and `k2_managed_guard`, and 0.6.0's
//! `app_controls`, `app_governed_spaces`, `app_member_channels` and
//! `app_mode_gate`. A flag K2 doesn't gate on is kept in the list and
//! otherwise ignored.

use rusqlite::{params, Connection, OptionalExtension};

pub const APP_SCOPED_SESSIONS: &str = "app_scoped_sessions";
pub const HOME_CLIENT: &str = "home_client";
pub const APP_ORIGIN: &str = "app_origin";
/// Not in Sew 0.6.1 (asked of Sew, 0.46.8): `origins` in the App settings,
/// every public `wss://` origin of an App. Sew's request type denies unknown
/// fields, so the list goes only once Sew reports this flag.
pub const APP_ORIGINS: &str = "app_origins";
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

// ── Sew 0.7.0 (sew-56 v1.1d §5; K2 voice rooms, PRD v1.9 §6.6) ────────

/// SW-1: the server-governed K2 Admin Space (`spaces/create` with `owner:
/// "k2:server"`, `governance: "server"`; the level sync and `channels/bulk`
/// with `app_id` omitted).
pub const SERVER_GOVERNED_SPACE: &str = "server_governed_space";
/// SW-3: `GET /mgmt/v1/spaces/{id}/voice/states` and `voice/changes`.
pub const MGMT_VOICE_STATES: &str = "mgmt_voice_states";
/// SW-4: `POST /mgmt/v1/channels/{id}/closing`.
pub const VOICE_CHANNEL_CLOSING: &str = "voice_channel_closing";
/// SW-9: relay-only through the edge (`voice.ops.ice_relay_only`).
pub const TURN_RELAY_ONLY: &str = "turn_relay_only";
/// SW-11: `POST /mgmt/v1/voice/media-path` (K2 calls it only with this).
pub const VOICE_MEDIA_PATH: &str = "voice_media_path";
/// SW-8: `sew import --to-standalone --identity-map` (move off K2).
pub const IMPORT_IDENTITY_MAP: &str = "import_identity_map";

/// Sew 0.7.0's six voice flags, read together from ONE feature list (the
/// one the last status refresh recorded, [`note`]), so a caller never
/// mixes two refreshes. Lanes gate on these fields, never on versions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct VoiceFeatures {
    /// SW-1 [`SERVER_GOVERNED_SPACE`].
    pub server_governed_space: bool,
    /// SW-3 [`MGMT_VOICE_STATES`].
    pub mgmt_voice_states: bool,
    /// SW-4 [`VOICE_CHANNEL_CLOSING`].
    pub voice_channel_closing: bool,
    /// SW-9 [`TURN_RELAY_ONLY`].
    pub turn_relay_only: bool,
    /// SW-11 [`VOICE_MEDIA_PATH`].
    pub voice_media_path: bool,
    /// SW-8 [`IMPORT_IDENTITY_MAP`].
    pub import_identity_map: bool,
}

impl VoiceFeatures {
    /// From a feature list as Sew reported it (any order; unknown flags
    /// ignored).
    pub fn from_flags<S: AsRef<str>>(flags: &[S]) -> VoiceFeatures {
        let has = |f: &str| flags.iter().any(|x| x.as_ref() == f);
        VoiceFeatures {
            server_governed_space: has(SERVER_GOVERNED_SPACE),
            mgmt_voice_states: has(MGMT_VOICE_STATES),
            voice_channel_closing: has(VOICE_CHANNEL_CLOSING),
            turn_relay_only: has(TURN_RELAY_ONLY),
            voice_media_path: has(VOICE_MEDIA_PATH),
            import_identity_map: has(IMPORT_IDENTITY_MAP),
        }
    }

    /// From the list the last status refresh recorded for `kind` (one read).
    pub fn read(conn: &Connection, kind: &str) -> Result<VoiceFeatures, String> {
        Ok(VoiceFeatures::from_flags(&list(conn, kind)?))
    }

    /// What the Admin Space and rooms need (SW-1, SW-4): the S1/S2 sweep
    /// does nothing without both.
    pub fn rooms_ready(&self) -> bool {
        self.server_governed_space && self.voice_channel_closing
    }
}

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

    /// The pinned Sew (0.6.1, whose flags are 0.6.0's) reports every flag
    /// K2 gates on: what it reports is kept as reported (sorted, nothing
    /// dropped, unknown flags included), and each gate answers yes.
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

    /// Sew 0.7.0's six voice flags (sew-56 v1.1d §5), read from one list.
    #[test]
    fn voice_features_read_from_one_status_refresh() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_schema(&conn).unwrap();
        // The 0.6.0 pin: none of them.
        let old: Vec<String> = SEW_0_6_0.iter().map(|s| s.to_string()).collect();
        note(&conn, "chat", &old, 1).unwrap();
        let v = VoiceFeatures::read(&conn, "chat").unwrap();
        assert_eq!(v, VoiceFeatures::default());
        assert!(!v.rooms_ready());
        // 0.7.0: all six, beside 0.6.0's twenty.
        let mut new = old.clone();
        for f in [SERVER_GOVERNED_SPACE, MGMT_VOICE_STATES, VOICE_CHANNEL_CLOSING, TURN_RELAY_ONLY, VOICE_MEDIA_PATH, IMPORT_IDENTITY_MAP] {
            new.push(f.to_string());
        }
        assert!(note(&conn, "chat", &new, 2).unwrap());
        let v = VoiceFeatures::read(&conn, "chat").unwrap();
        assert_eq!(
            v,
            VoiceFeatures {
                server_governed_space: true,
                mgmt_voice_states: true,
                voice_channel_closing: true,
                turn_relay_only: true,
                voice_media_path: true,
                import_identity_map: true,
            }
        );
        assert!(v.rooms_ready());
        // The exact strings Sew lists (never renamed).
        assert_eq!(
            [SERVER_GOVERNED_SPACE, MGMT_VOICE_STATES, VOICE_CHANNEL_CLOSING, TURN_RELAY_ONLY, VOICE_MEDIA_PATH, IMPORT_IDENTITY_MAP],
            ["server_governed_space", "mgmt_voice_states", "voice_channel_closing", "turn_relay_only", "voice_media_path", "import_identity_map"]
        );
        // One flag alone sets only its field.
        let one = VoiceFeatures::from_flags(&["voice_media_path"]);
        assert!(one.voice_media_path && !one.server_governed_space && !one.rooms_ready());
    }
}
