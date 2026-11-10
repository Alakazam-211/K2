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
//! | [`APP_RECORDING`], [`VOICE_RECORD_CAP`], [`RECORDING_REQUIRED`], [`MGMT_RECORDING_READS`] | Sew 0.9.0 (recording in Apps): read together as [`RecordingFeatures`] (`tool_hook::sew_recording_features()`). Sew 0.8.x reports none, so every recording change is inert there |
//!
//! The pin (Sew 0.8.2) reports thirty-one flags: 0.8.0's and 0.8.1's thirty, which are 0.7.3's
//! twenty-nine (the same as 0.7.2's) and `app_participant_links`, plus 0.8.2's `principal_erase`
//! (`POST /mgmt/v1/principals/erase`). K2 keeps both and uses neither yet. Up to 0.6.1 they are 0.5.0's
//! twelve, 0.5.1's `app_session_member_role`, `app_voice_channel_create`,
//! `home_client_owner_approval` and `k2_managed_guard`, and 0.6.0's
//! `app_controls`, `app_governed_spaces`, `app_member_channels` and
//! `app_mode_gate`. A flag K2 doesn't gate on is kept in the list and
//! otherwise ignored.

use rusqlite::{params, Connection, OptionalExtension};

pub const APP_SCOPED_SESSIONS: &str = "app_scoped_sessions";
pub const HOME_CLIENT: &str = "home_client";
pub const APP_ORIGIN: &str = "app_origin";
/// Sew 0.7.2 (not 0.7.1 or older): `origins` in the App settings,
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

// ── Sew 0.9.0 (recording in Apps, K2 PRD prd-k2-bobbin-in-apps-v1) ────

/// An App's `recording {start_level, required, server_read, audio_max_days}`
/// settings, the recorder install, and the kit door's recording reads and
/// fields (`recording_rule`, `recording_required`, `transcript_ready`).
pub const APP_RECORDING: &str = "app_recording";
/// The `voice:record` pass cap (bit 32) and an App's
/// `recording.cap_required`. Without it K2 drops `voice:record` from every
/// pass (an older Sew refuses a pass with an unknown cap).
pub const VOICE_RECORD_CAP: &str = "voice_record_cap";
/// Space and K5 recording overrides (`recording_start_level`,
/// `recording_required` in `spaces/{id}/settings`) and the org policy
/// `recording.required` (`off | multi_person`).
pub const RECORDING_REQUIRED: &str = "recording_required";
/// The mgmt recording reads (`recordings/{id}`, `…/transcript`,
/// `spaces/{id}/recordings`) and the App feed (`apps/{id}/recordings/changes`).
pub const MGMT_RECORDING_READS: &str = "mgmt_recording_reads";

/// Sew 0.9.0's four recording flags, read together from ONE feature list
/// (like [`VoiceFeatures`]). Gates read these fields, never versions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingFeatures {
    /// [`APP_RECORDING`].
    pub app_recording: bool,
    /// [`VOICE_RECORD_CAP`].
    pub voice_record_cap: bool,
    /// [`RECORDING_REQUIRED`].
    pub recording_required: bool,
    /// [`MGMT_RECORDING_READS`].
    pub mgmt_recording_reads: bool,
}

impl RecordingFeatures {
    /// From a feature list as Sew reported it (any order; unknown flags
    /// ignored).
    pub fn from_flags<S: AsRef<str>>(flags: &[S]) -> RecordingFeatures {
        let has = |f: &str| flags.iter().any(|x| x.as_ref() == f);
        RecordingFeatures {
            app_recording: has(APP_RECORDING),
            voice_record_cap: has(VOICE_RECORD_CAP),
            recording_required: has(RECORDING_REQUIRED),
            mgmt_recording_reads: has(MGMT_RECORDING_READS),
        }
    }

    /// From the list the last status refresh recorded for `kind` (one read).
    pub fn read(conn: &Connection, kind: &str) -> Result<RecordingFeatures, String> {
        Ok(RecordingFeatures::from_flags(&list(conn, kind)?))
    }

    /// Any of the four: this Sew can record in Apps at all.
    pub fn any(&self) -> bool {
        self.app_recording || self.voice_record_cap || self.recording_required || self.mgmt_recording_reads
    }
}

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

    /// Sew 0.6.0's and 0.6.1's flags report every flag K2 gated on then: what
    /// it reports is kept as reported (sorted, nothing dropped, unknown flags
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

    /// The pin, Sew 0.7.2: exactly `sew_proto::mgmt::FEATURES` at the v0.7.2 tag
    /// (`e49e088d…`, its `features_072.rs`): 0.6.1's twenty, 0.7.0's six, 0.7.1's
    /// two, 0.7.2's `app_origins`. Every voice gate opens (only the preview keeps
    /// them dark), and K2 sends every origin of an App. The pin, Sew 0.7.3,
    /// reports exactly these twenty-nine (its release note: no new features).
    #[test]
    fn sew_0_7_2_flags_open_the_voice_gates_and_take_every_origin() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_schema(&conn).unwrap();
        let mut reported: Vec<String> = SEW_0_6_0.iter().map(|s| s.to_string()).collect();
        for f in [
            "server_governed_space",
            "import_identity_map",
            "mgmt_voice_states",
            "voice_channel_closing",
            "voice_media_path",
            "turn_relay_only",
            "call_credential",
            "call_stats_native",
            "app_origins",
        ] {
            reported.push(f.to_string());
        }
        assert!(note(&conn, "chat", &reported, 1).unwrap());
        assert_eq!(list(&conn, "chat").unwrap().len(), 29, "kept whole");
        assert!(VoiceFeatures::read(&conn, "chat").unwrap().rooms_ready());
        for flag in [APP_ORIGIN, APP_ORIGINS, APP_GOVERNED_SPACES, HOME_CLIENT_OWNER_APPROVAL, K2_MANAGED_AVATAR, "k2_managed_guard"] {
            assert!(has(&conn, "chat", flag).unwrap(), "{flag}");
        }
    }

    /// Sew 0.8.0 and 0.8.1 (no server change between them): 0.7.3's twenty-nine plus `app_participant_links`
    /// (sew-58's participant links, its `features_080.rs`). K2 doesn't make links
    /// yet, so the flag is kept and ignored, and every gate reads as with 0.7.3.
    #[test]
    fn sew_0_8_0_flags_keep_every_gate_and_the_new_flag() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_schema(&conn).unwrap();
        let mut reported: Vec<String> = SEW_0_6_0.iter().map(|s| s.to_string()).collect();
        for f in [
            "server_governed_space",
            "import_identity_map",
            "mgmt_voice_states",
            "voice_channel_closing",
            "voice_media_path",
            "turn_relay_only",
            "call_credential",
            "call_stats_native",
            "app_origins",
            "app_participant_links",
        ] {
            reported.push(f.to_string());
        }
        assert!(note(&conn, "chat", &reported, 1).unwrap());
        assert_eq!(list(&conn, "chat").unwrap().len(), 30, "kept whole");
        assert!(VoiceFeatures::read(&conn, "chat").unwrap().rooms_ready());
        for flag in [APP_ORIGIN, APP_ORIGINS, APP_GOVERNED_SPACES, HOME_CLIENT_OWNER_APPROVAL, K2_MANAGED_AVATAR, "k2_managed_guard", "app_participant_links"] {
            assert!(has(&conn, "chat", flag).unwrap(), "{flag}");
        }
    }

    /// The pin, Sew 0.8.2: 0.8.1's thirty plus `principal_erase` (account
    /// erase, `POST /mgmt/v1/principals/erase`). K2 doesn't erase accounts
    /// through Sew yet, so the flag is kept and ignored, and every gate reads
    /// as with 0.8.1.
    #[test]
    fn sew_0_8_2_flags_keep_every_gate_and_the_erase_flag() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_schema(&conn).unwrap();
        let mut reported: Vec<String> = SEW_0_6_0.iter().map(|s| s.to_string()).collect();
        for f in [
            "server_governed_space",
            "import_identity_map",
            "mgmt_voice_states",
            "voice_channel_closing",
            "voice_media_path",
            "turn_relay_only",
            "call_credential",
            "call_stats_native",
            "app_origins",
            "app_participant_links",
            "principal_erase",
        ] {
            reported.push(f.to_string());
        }
        assert!(note(&conn, "chat", &reported, 1).unwrap());
        assert_eq!(list(&conn, "chat").unwrap().len(), 31, "kept whole");
        assert!(VoiceFeatures::read(&conn, "chat").unwrap().rooms_ready());
        for flag in [APP_ORIGIN, APP_ORIGINS, APP_GOVERNED_SPACES, HOME_CLIENT_OWNER_APPROVAL, K2_MANAGED_AVATAR, "k2_managed_guard", "app_participant_links", "principal_erase"] {
            assert!(has(&conn, "chat", flag).unwrap(), "{flag}");
        }
    }

    /// Sew 0.8.2's thirty-one flags, as the pin reports them.
    fn sew_0_8_2() -> Vec<String> {
        let mut v: Vec<String> = SEW_0_6_0.iter().map(|s| s.to_string()).collect();
        for f in [
            "server_governed_space",
            "import_identity_map",
            "mgmt_voice_states",
            "voice_channel_closing",
            "voice_media_path",
            "turn_relay_only",
            "call_credential",
            "call_stats_native",
            "app_origins",
            "app_participant_links",
            "principal_erase",
        ] {
            v.push(f.to_string());
        }
        v
    }

    /// Sew 0.9.0 (K2 PRD prd-k2-bobbin-in-apps-v1 §8, K-h prep): 0.8.2's
    /// thirty-one flags open no recording gate; 0.9.0's thirty-five (Sew's
    /// `features_090`) open all four, and each flag alone sets only its field.
    #[test]
    fn recording_flags() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_schema(&conn).unwrap();
        let old = sew_0_8_2();
        assert_eq!(old.len(), 31);
        note(&conn, "chat", &old, 1).unwrap();
        let r = RecordingFeatures::read(&conn, "chat").unwrap();
        assert_eq!(r, RecordingFeatures::default(), "Sew 0.8.2: inert");
        assert!(!r.any());
        let mut new = old.clone();
        for f in ["mgmt_recording_reads", "recording_required", "voice_record_cap", "app_recording"] {
            new.push(f.to_string());
        }
        assert!(note(&conn, "chat", &new, 2).unwrap());
        assert_eq!(list(&conn, "chat").unwrap().len(), 35, "features_090 pins 35");
        let r = RecordingFeatures::read(&conn, "chat").unwrap();
        assert_eq!(
            r,
            RecordingFeatures { app_recording: true, voice_record_cap: true, recording_required: true, mgmt_recording_reads: true }
        );
        // Sew's exact strings (never renamed).
        assert_eq!(
            [APP_RECORDING, VOICE_RECORD_CAP, RECORDING_REQUIRED, MGMT_RECORDING_READS],
            ["app_recording", "voice_record_cap", "recording_required", "mgmt_recording_reads"]
        );
        let one = RecordingFeatures::from_flags(&["voice_record_cap"]);
        assert!(one.voice_record_cap && !one.app_recording && !one.recording_required && !one.mgmt_recording_reads);
        // Voice gates are untouched by the new flags.
        assert!(VoiceFeatures::read(&conn, "chat").unwrap().rooms_ready());
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
