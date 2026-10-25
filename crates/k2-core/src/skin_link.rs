//! App link passes (`prd-k2-app-runtime-v1` §5.1–5.2, S14, AR9/AR50).
//!
//! A guarded App reaches K2 only through its **link**: one daemon
//! listener per App (`apps::gateway` in k2-daemon). The App proves itself
//! with a `k2skn_` pass the daemon mints for that link and nothing else:
//!
//! - a `tokens` row (platform shape, `session = 0`) with `app_service_id` =
//!   the App's published service id and `link_id` = the listener it is
//!   bound to (skin.db step 4);
//! - at most one hour to live (`expires_at`), rotated by the daemon at 80%;
//!   the previous pass stays valid until its own expiry, so a rotation
//!   never cuts a request in flight;
//! - **refused everywhere but its own link**: [`super::resolve_skin_token`]
//!   (the main port, the tunnel, every browser helper) never returns a row
//!   with a `link_id`, and [`resolve_link_pass`] returns one only for the
//!   link it names and the App it names;
//! - revoked (`revoked_at`) when the App stops or is uninstalled;
//! - never listed by `k2 app token list` and never re-roomed by
//!   `k2 app token rooms` (the daemon owns these rows).
//!
//! The remote App pass (an App hosted elsewhere, AR10, `link_id` NULL with
//! an App binding and an expiry) is the 0.46.1 half; it reuses this shape.

use rusqlite::{params, OptionalExtension};

use super::{
    caps_from_json, caps_json, display_prefix, generate_raw_key, is_api_key_prefix, is_skin_token, normalize_room_ids,
    now_secs, rooms_from_json, rooms_json, sha256_hex, with_conn, RoomPolicy, SkinPass, CAP_FILES_AGENT_CONFIG,
    CAP_FILES_WRITE, ACCEPTED_CAPS,
};

/// A link pass lives at most this long.
pub const LINK_PASS_TTL_SECS: i64 = 3600;
/// The daemon rotates a link pass once it is this old (80% of the TTL).
pub const LINK_PASS_ROTATE_AFTER_SECS: i64 = LINK_PASS_TTL_SECS * 4 / 5;
/// Expired link-pass rows are deleted after this long (nothing reads them).
pub const LINK_PASS_KEEP_EXPIRED_SECS: i64 = 86_400;
/// `tokens.name` prefix of a link pass. `:` never appears in a name a
/// human picks (`normalize_username`), so no platform token collides.
pub const LINK_PASS_NAME_PREFIX: &str = "applink:";

/// One minted link pass. `raw` is the secret, given to the App once.
#[derive(Debug, Clone)]
pub struct LinkPassMint {
    pub id: String,
    pub raw: String,
    pub prefix: String,
    pub caps: Vec<String>,
    pub rooms: Vec<String>,
    pub issued_at: i64,
    pub expires_at: i64,
}

/// What a presented `k2skn_` pass is, for the link's refusal codes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresentedPass {
    /// Not a live pass at all (unknown, revoked, expired, wrong prefix).
    Unknown,
    /// A live link pass for `link_id` (maybe another App's link).
    Link { link_id: String, app_service_id: String },
    /// A live pass that is not a link pass (a guest session, a platform
    /// token): never valid on a link.
    Other,
}

/// Caps an App's link pass may carry: the App's manifest ceiling, kept to
/// the caps K2 knows, minus `files:agentconfig` (reaching the agent's
/// `.k2/` folder stays an owner widening, S20). `files:write` is the only
/// cap with a companion rule; nothing here can add a cap the manifest
/// didn't ask for.
pub fn link_caps(manifest_caps: &[String]) -> (Vec<String>, Vec<String>) {
    let mut keep: Vec<String> = Vec::new();
    let mut dropped: Vec<String> = Vec::new();
    for c in manifest_caps {
        let t = c.trim();
        if t.is_empty() {
            continue;
        }
        if t == CAP_FILES_AGENT_CONFIG || !(ACCEPTED_CAPS.contains(&t) || super::APP_ONLY_CAPS.contains(&t)) {
            if !dropped.iter().any(|d| d == t) {
                dropped.push(t.to_string());
            }
            continue;
        }
        if !keep.iter().any(|k| k == t) {
            keep.push(t.to_string());
        }
    }
    debug_assert!(!keep.iter().any(|c| c == CAP_FILES_AGENT_CONFIG) || keep.iter().any(|c| c == CAP_FILES_WRITE));
    (keep, dropped)
}

/// Mint a pass for `link_id`, bound to the App `app_service_id`, with
/// `caps` (already [`link_caps`]-filtered) in `rooms`. Refuses an empty
/// binding; an empty cap list is allowed (the App can still call `/gw/me`).
pub fn mint_link_pass(
    app_service_id: &str,
    app_label: &str,
    link_id: &str,
    caps: &[String],
    rooms: &[String],
) -> Result<LinkPassMint, String> {
    let svc = app_service_id.trim();
    let link = link_id.trim();
    if svc.is_empty() || link.is_empty() {
        return Err("a link pass needs an App and a link".into());
    }
    let (caps, dropped) = link_caps(caps);
    if !dropped.is_empty() {
        return Err(format!("caps a link pass can't carry: {}", dropped.join(", ")));
    }
    let rooms = normalize_room_ids(rooms);
    let id = uuid::Uuid::new_v4().to_string();
    let raw = generate_raw_key();
    let prefix = display_prefix(&raw);
    let issued_at = now_secs();
    let expires_at = issued_at + LINK_PASS_TTL_SECS;
    let name = format!("{LINK_PASS_NAME_PREFIX}{}:{}", app_label.trim(), &id[..8]);
    with_conn(|conn| {
        conn.execute(
            "INSERT INTO tokens
             (id, principal_id, name, key_hash, key_prefix, caps, rooms, created_at, revoked_at, session, expires_at,
              app_service_id, link_id)
             VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6, ?7, NULL, 0, ?8, ?9, ?10)",
            params![id, name, sha256_hex(&raw), prefix, caps_json(&caps), rooms_json(&rooms), issued_at, expires_at, svc, link],
        )
        .map_err(|e| format!("link pass insert: {e}"))?;
        Ok(())
    })?;
    Ok(LinkPassMint { id, raw, prefix, caps, rooms, issued_at, expires_at })
}

/// Resolve `raw` presented on the link `link_id` of the App
/// `app_service_id`. `None` unless it is a live pass minted for exactly
/// that link and App (AR50: the pass's App is the listener's App).
pub fn resolve_link_pass(raw: &str, link_id: &str, app_service_id: &str) -> Option<SkinPass> {
    if !is_skin_token(raw) || is_api_key_prefix(raw) || link_id.trim().is_empty() || app_service_id.trim().is_empty() {
        return None;
    }
    let hash = sha256_hex(raw);
    let now = now_secs();
    with_conn(|conn| {
        conn.query_row(
            "SELECT id, name, caps, rooms FROM tokens
             WHERE key_hash = ?1 AND session = 0 AND revoked_at IS NULL
               AND expires_at IS NOT NULL AND expires_at > ?2
               AND link_id = ?3 AND app_service_id = ?4",
            params![hash, now, link_id, app_service_id],
            |r| {
                let caps = caps_from_json(&r.get::<_, String>(2)?);
                let rooms = rooms_from_json(&r.get::<_, String>(3)?);
                Ok(SkinPass {
                    id: r.get(0)?,
                    principal_id: None,
                    username: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    caps,
                    rooms,
                    session: false,
                    room_policy: RoomPolicy::new(),
                    app: app_service_id.to_string(),
                    room_paths: Default::default(),
                })
            },
        )
        .optional()
        .map_err(|e| format!("link pass resolve: {e}"))
    })
    .ok()
    .flatten()
}

/// Classify a presented pass (for `pass_wrong_link` vs `pass_invalid`).
pub fn classify_presented(raw: &str) -> PresentedPass {
    if !is_skin_token(raw) || is_api_key_prefix(raw) {
        return PresentedPass::Unknown;
    }
    let hash = sha256_hex(raw);
    let now = now_secs();
    let row = with_conn(|conn| {
        conn.query_row(
            "SELECT link_id, app_service_id FROM tokens
             WHERE key_hash = ?1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > ?2)",
            params![hash, now],
            |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)),
        )
        .optional()
        .map_err(|e| format!("pass classify: {e}"))
    })
    .ok()
    .flatten();
    match row {
        None => PresentedPass::Unknown,
        Some((Some(link_id), app)) => PresentedPass::Link { link_id, app_service_id: app.unwrap_or_default() },
        Some((None, _)) => PresentedPass::Other,
    }
}

/// Revoke every live link pass of an App (stop, uninstall). Returns how many.
pub fn revoke_link_passes(app_service_id: &str) -> Result<usize, String> {
    let svc = app_service_id.trim();
    if svc.is_empty() {
        return Err("missing App".into());
    }
    with_conn(|conn| {
        conn.execute(
            "UPDATE tokens SET revoked_at = ?1
             WHERE link_id IS NOT NULL AND app_service_id = ?2 AND revoked_at IS NULL",
            params![now_secs(), svc],
        )
        .map_err(|e| format!("link pass revoke: {e}"))
    })
}

/// Revoke one link pass by id (a rotation that couldn't be delivered).
pub fn revoke_link_pass(id: &str) -> Result<bool, String> {
    with_conn(|conn| {
        conn.execute(
            "UPDATE tokens SET revoked_at = ?1 WHERE id = ?2 AND link_id IS NOT NULL AND revoked_at IS NULL",
            params![now_secs(), id],
        )
        .map(|n| n > 0)
        .map_err(|e| format!("link pass revoke: {e}"))
    })
}

/// Live (unrevoked, unexpired) link passes of an App.
pub fn live_link_passes(app_service_id: &str) -> Result<usize, String> {
    let now = now_secs();
    with_conn(|conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM tokens
             WHERE link_id IS NOT NULL AND app_service_id = ?1 AND revoked_at IS NULL AND expires_at > ?2",
            params![app_service_id, now],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n.max(0) as usize)
        .map_err(|e| format!("link pass count: {e}"))
    })
}

/// Delete link-pass rows that expired or were revoked more than a day ago
/// (a running App leaves 24 rows a day). Returns how many went.
pub fn prune_link_passes() -> Result<usize, String> {
    let cutoff = now_secs() - LINK_PASS_KEEP_EXPIRED_SECS;
    with_conn(|conn| {
        conn.execute(
            "DELETE FROM tokens
             WHERE link_id IS NOT NULL
               AND ((expires_at IS NOT NULL AND expires_at < ?1) OR (revoked_at IS NOT NULL AND revoked_at < ?1))",
            params![cutoff],
        )
        .map_err(|e| format!("link pass prune: {e}"))
    })
}

// ── remote App passes (AR10) ─────────────────────────────────────────
//
// An App hosted elsewhere (Argus, shape c) can't be link-bound. Its pass
// reuses the same `tokens` columns: `app_service_id` = the App, `link_id`
// NULL, `expires_at` set, name `appremote:<label>:<id8>`. So:
// - every link listener refuses it ([`resolve_link_pass`] wants this
//   link's id): `pass_wrong_link`;
// - the general resolver accepts it (main port, tunnel) like a platform
//   token, until it expires or is revoked;
// - suspend and `k2 app token rotate` revoke it.

/// `tokens.name` prefix of a remote App pass.
pub const REMOTE_PASS_NAME_PREFIX: &str = "appremote:";
/// Default life of a remote pass (rotate with `k2 app token rotate`).
pub const REMOTE_PASS_TTL_SECS: i64 = 30 * 86_400;
/// Longest life a remote pass may be minted with.
pub const REMOTE_PASS_MAX_TTL_SECS: i64 = 90 * 86_400;

/// One remote pass as listed (never the secret).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePassInfo {
    pub id: String,
    pub prefix: String,
    pub caps: Vec<String>,
    pub rooms: Vec<String>,
    pub issued_at: i64,
    pub expires_at: i64,
    pub revoked_at: Option<i64>,
}

/// Is `name` (a `tokens.name`, the pass's stamp) a remote App pass?
pub fn is_remote_pass_name(name: &str) -> bool {
    name.starts_with(REMOTE_PASS_NAME_PREFIX)
}

/// Mint a remote pass for the App `app_service_id` with `caps` (filtered
/// like a link pass) in `rooms`, living `ttl_secs` (clamped to
/// 60 s … [`REMOTE_PASS_MAX_TTL_SECS`]).
pub fn mint_remote_pass(
    app_service_id: &str,
    app_label: &str,
    caps: &[String],
    rooms: &[String],
    ttl_secs: i64,
) -> Result<LinkPassMint, String> {
    let svc = app_service_id.trim();
    if svc.is_empty() {
        return Err("a remote App pass needs an App".into());
    }
    let (caps, dropped) = link_caps(caps);
    if !dropped.is_empty() {
        return Err(format!("caps a remote App pass can't carry: {}", dropped.join(", ")));
    }
    let rooms = normalize_room_ids(rooms);
    let id = uuid::Uuid::new_v4().to_string();
    let raw = generate_raw_key();
    let prefix = display_prefix(&raw);
    let issued_at = now_secs();
    let expires_at = issued_at + ttl_secs.clamp(60, REMOTE_PASS_MAX_TTL_SECS);
    let name = format!("{REMOTE_PASS_NAME_PREFIX}{}:{}", app_label.trim(), &id[..8]);
    with_conn(|conn| {
        conn.execute(
            "INSERT INTO tokens
             (id, principal_id, name, key_hash, key_prefix, caps, rooms, created_at, revoked_at, session, expires_at,
              app_service_id, link_id)
             VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6, ?7, NULL, 0, ?8, ?9, NULL)",
            params![id, name, sha256_hex(&raw), prefix, caps_json(&caps), rooms_json(&rooms), issued_at, expires_at, svc],
        )
        .map_err(|e| format!("remote pass insert: {e}"))?;
        Ok(())
    })?;
    Ok(LinkPassMint { id, raw, prefix, caps, rooms, issued_at, expires_at })
}

/// An App's remote passes, newest first (live and recently ended).
pub fn remote_passes(app_service_id: &str) -> Result<Vec<RemotePassInfo>, String> {
    let like = format!("{REMOTE_PASS_NAME_PREFIX}%");
    with_conn(|conn| {
        let mut st = conn
            .prepare(
                "SELECT id, key_prefix, caps, rooms, created_at, expires_at, revoked_at FROM tokens
                 WHERE link_id IS NULL AND session = 0 AND app_service_id = ?1 AND name LIKE ?2
                 ORDER BY created_at DESC, id",
            )
            .map_err(|e| format!("remote pass list: {e}"))?;
        let rows = st
            .query_map(params![app_service_id, like], |r| {
                Ok(RemotePassInfo {
                    id: r.get(0)?,
                    prefix: r.get(1)?,
                    caps: caps_from_json(&r.get::<_, String>(2)?),
                    rooms: rooms_from_json(&r.get::<_, String>(3)?),
                    issued_at: r.get(4)?,
                    expires_at: r.get::<_, Option<i64>>(5)?.unwrap_or(0),
                    revoked_at: r.get(6)?,
                })
            })
            .map_err(|e| format!("remote pass list: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("remote pass list: {e}"))
    })
}

/// The App a live remote pass (by token id) is bound to.
pub fn remote_pass_app(token_id: &str) -> Option<String> {
    let like = format!("{REMOTE_PASS_NAME_PREFIX}%");
    with_conn(|conn| {
        conn.query_row(
            "SELECT app_service_id FROM tokens WHERE id = ?1 AND link_id IS NULL AND session = 0 AND name LIKE ?2",
            params![token_id, like],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(|e| format!("remote pass app: {e}"))
    })
    .ok()
    .flatten()
    .flatten()
}

/// Revoke an App's live remote passes, or one of them (`only`). Returns
/// how many.
pub fn revoke_remote_passes(app_service_id: &str, only: Option<&str>) -> Result<usize, String> {
    let svc = app_service_id.trim();
    if svc.is_empty() {
        return Err("missing App".into());
    }
    let like = format!("{REMOTE_PASS_NAME_PREFIX}%");
    with_conn(|conn| {
        conn.execute(
            "UPDATE tokens SET revoked_at = ?1
             WHERE link_id IS NULL AND session = 0 AND app_service_id = ?2 AND name LIKE ?3 AND revoked_at IS NULL
               AND (?4 IS NULL OR id = ?4)",
            params![now_secs(), svc, like, only],
        )
        .map_err(|e| format!("remote pass revoke: {e}"))
    })
}

/// Suspend (§5.4): revoke every live pass the App holds, link and remote,
/// in one statement. Guest sessions are the guests' own and stay.
pub fn revoke_app_passes(app_service_id: &str) -> Result<usize, String> {
    let svc = app_service_id.trim();
    if svc.is_empty() {
        return Err("missing App".into());
    }
    let like = format!("{REMOTE_PASS_NAME_PREFIX}%");
    with_conn(|conn| {
        conn.execute(
            "UPDATE tokens SET revoked_at = ?1
             WHERE app_service_id = ?2 AND session = 0 AND revoked_at IS NULL
               AND (link_id IS NOT NULL OR name LIKE ?3)",
            params![now_secs(), svc, like],
        )
        .map_err(|e| format!("App pass revoke: {e}"))
    })
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;

    /// AR10: a remote pass is App-bound with an expiry; refused on every
    /// link listener; accepted by the general resolver; revoked by suspend
    /// (with the link passes) and by rotate.
    #[test]
    fn a_remote_pass_is_refused_on_links_and_revoked_by_suspend() {
        with_temp_home(|| {
            let r = mint_remote_pass(APP, "argus", &caps(&["thread:read", "pty:write"]), &["ws-1".to_string()], 3600);
            assert!(r.unwrap_err().contains("pty:write"), "unknown caps are refused, not dropped");
            let r = mint_remote_pass(APP, "argus", &caps(&["thread:read"]), &["ws-1".to_string()], 10 * REMOTE_PASS_MAX_TTL_SECS).unwrap();
            assert_eq!(r.expires_at - r.issued_at, REMOTE_PASS_MAX_TTL_SECS, "ttl clamped");
            let p = resolve_skin_token(&r.raw).expect("the general resolver accepts a remote pass");
            assert!(is_remote_pass_name(&p.username), "{}", p.username);
            assert_eq!(remote_pass_app(&p.id).as_deref(), Some(APP));
            assert!(resolve_link_pass(&r.raw, "gw:38601", APP).is_none(), "never on a link");
            assert_eq!(classify_presented(&r.raw), PresentedPass::Other);
            let listed = remote_passes(APP).unwrap();
            assert_eq!(listed.len(), 1);
            assert_eq!(listed[0].id, r.id);
            assert!(remote_passes(OTHER_APP).unwrap().is_empty());

            // Suspend: link and remote passes in one step; other tokens stay.
            let l = mint_link_pass(APP, "argus", "gw:38601", &caps(&["thread:read"]), &["ws-1".to_string()]).unwrap();
            let other = mint_remote_pass(OTHER_APP, "o", &caps(&["thread:read"]), &[], 3600).unwrap();
            let (_, plat) = create_token("dannon-bff", Some(&caps(&["thread:read"])), &["ws-1".to_string()]).unwrap();
            assert_eq!(revoke_app_passes(APP).unwrap(), 2);
            assert!(resolve_skin_token(&r.raw).is_none());
            assert!(resolve_link_pass(&l.raw, "gw:38601", APP).is_none());
            assert!(resolve_skin_token(&other.raw).is_some(), "another App's pass stays");
            assert!(resolve_skin_token(&plat).is_some(), "a platform token stays");
            assert!(remote_passes(APP).unwrap()[0].revoked_at.is_some());

            // Rotate = a new pass, then revoke the old one by id.
            let a = mint_remote_pass(OTHER_APP, "o", &caps(&["thread:read"]), &[], 3600).unwrap();
            assert_eq!(revoke_remote_passes(OTHER_APP, Some(&other.id)).unwrap(), 1);
            assert!(resolve_skin_token(&other.raw).is_none());
            assert!(resolve_skin_token(&a.raw).is_some());
            assert!(remote_pass_app(&resolve_skin_token(&plat).unwrap().id).is_none(), "a platform token isn't a remote pass");
        });
    }

    fn with_temp_home<F: FnOnce()>(f: F) {
        let _home = crate::test_env::TempHome::new();
        f();
    }

    const APP: &str = "6f1c0d7e-1111-4222-8333-944455556666";
    const OTHER_APP: &str = "7a2b0c1d-2222-4333-8444-955566667777";

    fn caps(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn link_caps_keep_the_manifest_ceiling_and_drop_what_a_link_cant_carry() {
        let (k, d) = link_caps(&caps(&["thread:read", "files:write", "files:agentconfig", "pty:write", "thread:read", " "]));
        assert_eq!(k, caps(&["thread:read", "files:write"]));
        assert_eq!(d, caps(&["files:agentconfig", "pty:write"]));
        assert_eq!(link_caps(&[]).0, Vec::<String>::new());
        // push:send is the App's own cap (custom-app-push-v1): a link pass
        // carries it; no guest role can (parse_caps refuses it).
        assert_eq!(link_caps(&caps(&["push:send"])).0, vec!["push:send".to_string()]);
        assert!(crate::skin::parse_caps(Some(&["push:send".to_string()])).is_err(), "never a role cap");
    }

    /// AR9/AR50/§5.2: a link pass works only on its own link and App,
    /// never through the general resolver (main port, tunnel, helpers),
    /// and stops working when revoked.
    #[test]
    fn a_link_pass_resolves_only_on_its_own_link() {
        with_temp_home(|| {
            let m = mint_link_pass(APP, "jobs", "gw:38601", &caps(&["thread:read"]), &["ws-1".to_string()]).unwrap();
            assert!(m.raw.starts_with(SKIN_KEY_PREFIX));
            assert_eq!(m.expires_at - m.issued_at, LINK_PASS_TTL_SECS);
            assert!(LINK_PASS_ROTATE_AFTER_SECS < LINK_PASS_TTL_SECS);

            let p = resolve_link_pass(&m.raw, "gw:38601", APP).expect("own link");
            assert_eq!(p.app, APP);
            assert!(!p.session);
            assert_eq!(p.caps, caps(&["thread:read"]));
            assert_eq!(p.rooms, vec!["ws-1".to_string()]);
            assert!(p.has_cap_in_room("ws-1", "thread:read"));
            assert!(!p.has_room("ws-2"));

            assert!(resolve_skin_token(&m.raw).is_none(), "the general resolver never accepts a link pass");
            assert!(resolve_link_pass(&m.raw, "gw:38602", APP).is_none(), "another link");
            assert!(resolve_link_pass(&m.raw, "gw:38601", OTHER_APP).is_none(), "another App");
            assert!(resolve_link_pass("k2skn_nope", "gw:38601", APP).is_none());
            assert_eq!(classify_presented(&m.raw), PresentedPass::Link { link_id: "gw:38601".into(), app_service_id: APP.into() });

            // A platform token is a live pass but never a link pass.
            let (_, plat) = create_token("dannon-bff", Some(&caps(&["thread:read"])), &["ws-1".to_string()]).unwrap();
            assert!(resolve_link_pass(&plat, "gw:38601", APP).is_none());
            assert_eq!(classify_presented(&plat), PresentedPass::Other);
            assert_eq!(classify_presented("k2skn_unknown"), PresentedPass::Unknown);

            // Not listed or re-roomed through the platform-token tools.
            assert!(list_tokens().unwrap().iter().all(|t| t.id != m.id), "k2 app token list never shows link passes");
            assert!(set_token_rooms(&m.id, &["ws-9".to_string()]).is_err(), "k2 app token rooms can't widen a link pass");
            assert_eq!(resolve_link_pass(&m.raw, "gw:38601", APP).unwrap().rooms, vec!["ws-1".to_string()]);

            // Rotation: a second pass; both live until revoke.
            let m2 = mint_link_pass(APP, "jobs", "gw:38601", &caps(&["thread:read"]), &["ws-1".to_string()]).unwrap();
            assert_eq!(live_link_passes(APP).unwrap(), 2);
            assert!(resolve_link_pass(&m.raw, "gw:38601", APP).is_some(), "the old pass lives until its own expiry");
            assert!(revoke_link_pass(&m.id).unwrap());
            assert!(resolve_link_pass(&m.raw, "gw:38601", APP).is_none());
            assert_eq!(revoke_link_passes(APP).unwrap(), 1);
            assert!(resolve_link_pass(&m2.raw, "gw:38601", APP).is_none());
            assert_eq!(classify_presented(&m2.raw), PresentedPass::Unknown);
            assert_eq!(live_link_passes(APP).unwrap(), 0);
            assert!(resolve_skin_token(&plat).is_some(), "revoking an App's link passes leaves other tokens alone");
        });
    }

    #[test]
    fn expired_link_passes_are_refused_and_pruned() {
        with_temp_home(|| {
            let m = mint_link_pass(APP, "jobs", "gw:38601", &caps(&["thread:read"]), &["ws-1".to_string()]).unwrap();
            with_conn(|c| {
                c.execute("UPDATE tokens SET expires_at = ?1 WHERE id = ?2", params![now_secs() - 1, m.id]).map_err(|e| e.to_string())
            })
            .unwrap();
            assert!(resolve_link_pass(&m.raw, "gw:38601", APP).is_none(), "expired");
            assert_eq!(prune_link_passes().unwrap(), 0, "kept for a day");
            with_conn(|c| {
                c.execute("UPDATE tokens SET expires_at = ?1 WHERE id = ?2", params![now_secs() - LINK_PASS_KEEP_EXPIRED_SECS - 5, m.id])
                    .map_err(|e| e.to_string())
            })
            .unwrap();
            assert_eq!(prune_link_passes().unwrap(), 1);
        });
    }

    #[test]
    fn mint_refuses_an_unbound_pass_and_extra_caps() {
        with_temp_home(|| {
            assert!(mint_link_pass("", "jobs", "gw:1", &[], &[]).is_err());
            assert!(mint_link_pass(APP, "jobs", " ", &[], &[]).is_err());
            assert!(mint_link_pass(APP, "jobs", "gw:1", &caps(&["files:agentconfig"]), &[]).is_err());
            assert!(mint_link_pass(APP, "jobs", "gw:1", &[], &[]).is_ok(), "no caps is fine: /gw/me still answers");
        });
    }
}
