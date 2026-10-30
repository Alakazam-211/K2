//! Companion C4 (prd-companion-v2 §4; feedback PRD §8.4 Option B) —
//! the daemon-held mobile push-device registry.
//!
//! One row per mobile device that asked THIS daemon to push to it.
//! The companion re-registers on every app launch (upsert keyed by a
//! stable `device_id` — which also absorbs APNs/FCM token rotation
//! for free); the relay gateway stays stateless and only ever sees
//! tokens in-flight on a dispatch call. The daemon's
//! `/cli/push/register-device` / `unregister-device` routes and the
//! [`crate::push::K2Cloud`] dispatch adapter are thin wrappers over
//! this module (daemon-first: all logic lives here).
//!
//! Error convention (project_groups.rs discipline): `Err(String)`
//! with a human hint; validation misses read as usage errors at the
//! route layer.

use rusqlite::params;

/// The vendor platforms the registry accepts. Anything else is
/// refused loudly at [`upsert_device`] time — a typo'd platform must
/// never sit in the table waiting to confuse the gateway.
pub const PLATFORMS: [&str; 2] = ["apns", "fcm"];

/// One `push_devices` row. Serializes camelCase — the wire shape the
/// C4 routes return.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushDevice {
    /// Stable app-install identity minted by the companion (the
    /// upsert key). NOT the vendor token, which rotates.
    pub device_id: String,
    /// `'apns'` | `'fcm'`.
    pub platform: String,
    /// The vendor routing handle (APNs device token / FCM
    /// registration token).
    pub token: String,
    /// Daemon-resolved authed user who registered the device
    /// (`'owner'` | a connect-user name) — resolved from the session
    /// token, never the request body (D3).
    pub username: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// Bumps on every (re-)register — staleness signal for a future
    /// list/revoke UI.
    pub last_seen_at: i64,
    /// Thread push message previews on this phone's lock screen (0.46.1,
    /// migration 0189). A NULL column (never set) reads as `true`. The
    /// owner's `pushMessagePreviews` switch still overrides it.
    pub previews: bool,
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

const DEVICE_SELECT: &str = "SELECT device_id, platform, token, username, \
    created_at, updated_at, last_seen_at, previews FROM push_devices";

fn row_to_device(row: &rusqlite::Row) -> rusqlite::Result<PushDevice> {
    Ok(PushDevice {
        device_id: row.get(0)?,
        platform: row.get(1)?,
        token: row.get(2)?,
        username: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
        last_seen_at: row.get(6)?,
        previews: row.get::<_, Option<i64>>(7)?.is_none_or(|v| v != 0),
    })
}

/// Register (or refresh) a device. Keyed by `device_id`: a re-register
/// updates platform/token/username and bumps `updated_at` +
/// `last_seen_at`, preserving `created_at` — the companion re-sends on
/// every app launch, which is also how APNs/FCM token rotation lands.
/// Keeps the stored `previews` choice ([`upsert_device_with`] sets it).
pub fn upsert_device(
    device_id: &str,
    platform: &str,
    token: &str,
    username: &str,
) -> Result<PushDevice, String> {
    upsert_device_with(device_id, platform, token, username, None)
}

/// [`upsert_device`] plus the phone's Thread preview choice: `Some(b)`
/// stores it; `None` keeps the stored value (an older phone that does not
/// send the field), and a new row then reads as on.
pub fn upsert_device_with(
    device_id: &str,
    platform: &str,
    token: &str,
    username: &str,
    previews: Option<bool>,
) -> Result<PushDevice, String> {
    upsert_device_for_session(device_id, platform, token, username, previews, None)
}

/// [`upsert_device_with`] plus the login session that registered the
/// device (0.46.3, migration 0195). `session_sid` is the caller's Connect
/// session id (`connect_users::session_sid`, `cu_…`) resolved by the
/// daemon — never a token, never the request body; `None` for the owner
/// token (no session). Every register REPLACES the link: a phone that
/// signed in again re-links to its new session, and a device that moves
/// to the owner token is no longer tied to any login. When the linked
/// session ends, [`remove_devices_for_sessions`] drops the row.
pub fn upsert_device_for_session(
    device_id: &str,
    platform: &str,
    token: &str,
    username: &str,
    previews: Option<bool>,
    session_sid: Option<&str>,
) -> Result<PushDevice, String> {
    upsert_device_e2e(
        device_id,
        platform,
        token,
        username,
        previews,
        session_sid,
        crate::push_device_keys::E2eChange::Keep,
    )
}

/// [`upsert_device_for_session`] plus the phone's push key
/// (prd-e2e-push-v1 rev 2 §7.1, migration 0211), in ONE transaction:
/// - a register that changes the device's login deletes the key row
///   first (V12: the old person's key must never stay attached);
/// - then `e2e`: [`Keep`](crate::push_device_keys::E2eChange::Keep) leaves
///   it, `Delete` removes it, `Set` stores it (the same key again is a
///   no-op).
pub fn upsert_device_e2e(
    device_id: &str,
    platform: &str,
    token: &str,
    username: &str,
    previews: Option<bool>,
    session_sid: Option<&str>,
    e2e: crate::push_device_keys::E2eChange,
) -> Result<PushDevice, String> {
    let session_sid = session_sid.map(str::trim).filter(|s| !s.is_empty());
    let device_id = device_id.trim();
    let token = token.trim();
    let username = username.trim();
    if device_id.is_empty() {
        return Err("usage: deviceId must not be empty".to_string());
    }
    if token.is_empty() {
        return Err("usage: token must not be empty".to_string());
    }
    if username.is_empty() {
        return Err("usage: username must not be empty".to_string());
    }
    if !PLATFORMS.contains(&platform) {
        return Err(format!(
            "usage: platform must be one of {} — got '{platform}'",
            PLATFORMS.join(", ")
        ));
    }
    let now = now_secs();
    // K5 (0.46.7): who this registration's person is, read before the DB
    // lock (the people list is a file).
    let people = crate::notify_prefs::People::load().ok();
    let person_of = |login: &str| match people.as_ref() {
        Some(p) => p.person_key(login),
        None => login.trim().to_ascii_lowercase(),
    };
    let me = person_of(username);
    let db = crate::db::shared();
    let conn = db.lock();
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("push device upsert failed: {e}"))?;
    // K5: the same vendor token under another deviceId for the same person
    // is the same install registered twice (an old id the phone dropped):
    // that person's every push reached the phone twice. The new id
    // replaces it.
    for dup in duplicate_token_rows(&tx, platform, token, device_id)?
        .into_iter()
        .filter(|(_, login)| person_of(login) == me)
        .map(|(id, _)| id)
    {
        crate::push_device_keys::delete_key_in(&tx, &dup)?;
        tx.execute("DELETE FROM push_devices WHERE device_id = ?1", params![dup])
            .map_err(|e| format!("push device duplicate delete failed: {e}"))?;
    }
    let prev_user: Option<String> = tx
        .query_row(
            "SELECT username FROM push_devices WHERE device_id = ?1",
            params![device_id],
            |r| r.get(0),
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(format!("push device upsert failed: {e}")),
        })?;
    tx.execute(
        "INSERT INTO push_devices (device_id, platform, token, username, \
         created_at, updated_at, last_seen_at, previews, session_sid) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?5, ?6, ?7) \
         ON CONFLICT(device_id) DO UPDATE SET \
         platform = excluded.platform, token = excluded.token, \
         username = excluded.username, updated_at = excluded.updated_at, \
         last_seen_at = excluded.last_seen_at, \
         previews = COALESCE(excluded.previews, push_devices.previews), \
         session_sid = excluded.session_sid",
        params![device_id, platform, token, username, now, previews.map(i64::from), session_sid],
    )
    .map_err(|e| format!("push device upsert failed: {e}"))?;
    if prev_user.as_deref().is_some_and(|u| u != username) {
        crate::push_device_keys::delete_key_in(&tx, device_id)?;
    }
    match e2e {
        crate::push_device_keys::E2eChange::Keep => {}
        crate::push_device_keys::E2eChange::Delete => {
            crate::push_device_keys::delete_key_in(&tx, device_id)?;
        }
        crate::push_device_keys::E2eChange::Set(key) => {
            crate::push_device_keys::put_key_in(&tx, device_id, &key, now)?;
        }
    }
    let device = tx
        .query_row(&format!("{DEVICE_SELECT} WHERE device_id = ?1"), params![device_id], row_to_device)
        .map_err(|e| format!("push device readback failed: {e}"))?;
    tx.commit().map_err(|e| format!("push device upsert failed: {e}"))?;
    Ok(device)
}

/// Other rows holding this platform's vendor token: `(device_id, username)`.
fn duplicate_token_rows(
    conn: &rusqlite::Connection,
    platform: &str,
    token: &str,
    device_id: &str,
) -> Result<Vec<(String, String)>, String> {
    let mut st = conn
        .prepare("SELECT device_id, username FROM push_devices WHERE platform = ?1 AND token = ?2 AND device_id != ?3")
        .map_err(|e| format!("push device duplicate check failed: {e}"))?;
    let rows = st
        .query_map(params![platform, token, device_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| format!("push device duplicate check failed: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("push device duplicate check failed: {e}"))
}

/// A short, stable fingerprint of a vendor token (K5, 0.46.7): the first 8
/// hex characters of SHA-256(token). Two rows with the same fingerprint
/// hold the same token. The token itself is never shown.
pub fn token_fingerprint(token: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(token.trim().as_bytes()).iter().take(4).map(|b| format!("{b:02x}")).collect()
}

/// Keep one row per vendor token (the most recently seen): a person's
/// phone must never get the same push twice because two of its rows share
/// a token (K5). Order is otherwise kept.
pub fn dedupe_by_token(devices: &[PushDevice]) -> Vec<PushDevice> {
    let mut best: std::collections::HashMap<(&str, &str), (i64, usize)> = std::collections::HashMap::new();
    for (i, d) in devices.iter().enumerate() {
        let k = (d.platform.as_str(), d.token.as_str());
        match best.get(&k) {
            Some((seen, _)) if *seen >= d.last_seen_at => {}
            _ => {
                best.insert(k, (d.last_seen_at, i));
            }
        }
    }
    devices
        .iter()
        .enumerate()
        .filter(|(i, d)| best.get(&(d.platform.as_str(), d.token.as_str())).is_some_and(|(_, keep)| keep == i))
        .map(|(_, d)| d.clone())
        .collect()
}

/// Delete the key rows of the devices a `WHERE` clause selects (V13: every
/// delete path deletes key rows explicitly; the FK is the second guard).
fn delete_keys_where(conn: &rusqlite::Connection, clause: &str, p: &dyn rusqlite::ToSql) -> Result<usize, String> {
    conn.execute(
        &format!("DELETE FROM push_device_keys WHERE device_id IN (SELECT device_id FROM push_devices WHERE {clause})"),
        [p],
    )
    .map_err(|e| format!("push key delete failed: {e}"))
}

/// Unregister a device by its id. `Ok(true)` when a row was removed,
/// `Ok(false)` when the id was unknown (an unregister of an already-
/// gone device is a fine no-op — the companion toggles blind).
pub fn remove_device(device_id: &str) -> Result<bool, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    crate::push_device_keys::delete_key_in(&conn, device_id)?;
    let n = conn
        .execute(
            "DELETE FROM push_devices WHERE device_id = ?1",
            params![device_id.trim()],
        )
        .map_err(|e| format!("push device delete failed: {e}"))?;
    Ok(n > 0)
}

/// One device by its id, or `None`.
pub fn get_device(device_id: &str) -> Result<Option<PushDevice>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    match conn.query_row(
        &format!("{DEVICE_SELECT} WHERE device_id = ?1"),
        params![device_id.trim()],
        row_to_device,
    ) {
        Ok(d) => Ok(Some(d)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(format!("push device read failed: {e}")),
    }
}

/// Every registered device, oldest-registered first (rowid tiebreak
/// for same-second registrations).
pub fn list_devices() -> Result<Vec<PushDevice>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare(&format!("{DEVICE_SELECT} ORDER BY created_at, rowid"))
        .map_err(|e| format!("push device list failed: {e}"))?;
    let rows = stmt
        .query_map([], row_to_device)
        .map_err(|e| format!("push device list failed: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("push device list failed: {e}"))
}

/// Devices whose `username` is in `usernames` (exact match). Used for
/// ticket-assignee push targeting. Empty input → empty result (caller
/// decides whether to fall back to [`list_devices`]).
pub fn list_devices_for_usernames(usernames: &[String]) -> Result<Vec<PushDevice>, String> {
    if usernames.is_empty() {
        return Ok(Vec::new());
    }
    let all = list_devices()?;
    let set: std::collections::HashSet<&str> = usernames.iter().map(|s| s.as_str()).collect();
    Ok(all
        .into_iter()
        .filter(|d| set.contains(d.username.as_str()))
        .collect())
}

/// The login session a device is linked to (migration 0195): `Some(sid)`
/// for a Connect session, `None` for an owner-token or legacy row.
/// `Err` when the device does not exist.
pub fn device_session_sid(device_id: &str) -> Result<Option<String>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT session_sid FROM push_devices WHERE device_id = ?1",
        params![device_id.trim()],
        |r| r.get::<_, Option<String>>(0),
    )
    .map_err(|e| format!("push device session read failed: {e}"))
}

/// Drop every device registered by one of these login sessions (`cu_…`
/// ids): the session was logged out, revoked or expired. Returns the
/// number of rows removed. Owner-token and legacy rows (NULL) never match.
pub fn remove_devices_for_sessions(
    conn: &rusqlite::Connection,
    session_sids: &[String],
) -> Result<usize, String> {
    let mut n = 0;
    for sid in session_sids {
        delete_keys_where(conn, "session_sid = ?1", sid)?;
        n += conn
            .execute("DELETE FROM push_devices WHERE session_sid = ?1", params![sid])
            .map_err(|e| format!("push device session cleanup failed: {e}"))?;
    }
    Ok(n)
}

/// The sweep backstop (boot + the session reaper's tick): drop rows whose
/// login is gone even though no end event reached us (a session file reset
/// or deleted, an epoch bump never presented again).
///
/// - A row linked to a session (`session_sid` set) goes when that id is
///   not in `live_sids`.
/// - A legacy row (NULL `session_sid`, registered before 0195) of a
///   Connect user (any `username` but `owner`) goes when that user has
///   no live session (`users_with_live_sessions`), or when it was last
///   registered at or before `legacy_cutoff` (unix seconds: now minus the
///   session lifetime, so the session it was registered with has ended;
///   sessions never refresh).
/// - Owner-token rows (`owner`, NULL) are never touched.
/// - A row registered at or after `taken_at` (unix seconds, when the
///   caller read the live session set) is skipped: its session may be
///   newer than the snapshot (a login + register racing the sweep).
///
/// Returns the number of rows removed.
pub fn prune_devices_without_live_session(
    conn: &rusqlite::Connection,
    live_sids: &std::collections::HashSet<String>,
    users_with_live_sessions: &std::collections::HashSet<String>,
    legacy_cutoff: i64,
    taken_at: i64,
) -> Result<usize, String> {
    let rows: Vec<(String, String, Option<String>, i64)> = {
        let mut stmt = conn
            .prepare(
                "SELECT device_id, username, session_sid, last_seen_at FROM push_devices \
                 WHERE updated_at < ?1",
            )
            .map_err(|e| format!("push device sweep failed: {e}"))?;
        let it = stmt
            .query_map(params![taken_at], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .map_err(|e| format!("push device sweep failed: {e}"))?;
        it.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("push device sweep failed: {e}"))?
    };
    let mut n = 0;
    for (device_id, username, sid, last_seen) in rows {
        let dead = match sid {
            Some(sid) => !live_sids.contains(&sid),
            None => {
                username != crate::notify_prefs::OWNER
                    && (!users_with_live_sessions.contains(&username) || last_seen <= legacy_cutoff)
            }
        };
        if dead {
            crate::push_device_keys::delete_key_in(conn, &device_id)?;
            n += conn
                .execute("DELETE FROM push_devices WHERE device_id = ?1", params![device_id])
                .map_err(|e| format!("push device sweep failed: {e}"))?;
        }
    }
    Ok(n)
}

/// Drop every device holding a vendor token the gateway reported dead
/// (`410 Unregistered` / FCM `NotRegistered` → `dead:true` in the
/// dispatch response). Returns the number of rows pruned. Keyed by
/// TOKEN — that's all the gateway knows.
pub fn prune_device_by_token(token: &str) -> Result<usize, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    delete_keys_where(&conn, "token = ?1", &token)?;
    conn.execute(
        "DELETE FROM push_devices WHERE token = ?1",
        params![token],
    )
    .map_err(|e| format!("push device prune failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Per-test unique ids: the k2-core test binary shares ONE
    /// in-memory DB across threads (db::init_for_tests), so every
    /// assertion filters to rows this test itself created.
    fn did(label: &str) -> String {
        format!("dev-{label}-{}", uuid::Uuid::new_v4())
    }

    fn tok(label: &str) -> String {
        format!("tok-{label}-{}", uuid::Uuid::new_v4())
    }

    #[test]
    fn upsert_inserts_then_updates_preserving_created_at() {
        let id = did("upsert");
        let t1 = tok("upsert-1");
        let d = upsert_device(&id, "apns", &t1, "owner").expect("insert");
        assert_eq!(d.device_id, id);
        assert_eq!(d.platform, "apns");
        assert_eq!(d.token, t1);
        assert_eq!(d.username, "owner");
        assert!(d.created_at > 0);
        assert_eq!(d.created_at, d.updated_at);
        assert_eq!(d.created_at, d.last_seen_at);

        // Re-register with a rotated token, new platform and user:
        // same row (device_id key), created_at preserved.
        let t2 = tok("upsert-2");
        let d2 = upsert_device(&id, "fcm", &t2, "rosson").expect("update");
        assert_eq!(d2.device_id, id);
        assert_eq!(d2.platform, "fcm");
        assert_eq!(d2.token, t2, "token rotation lands via the upsert");
        assert_eq!(d2.username, "rosson");
        assert_eq!(d2.created_at, d.created_at, "created_at survives re-register");
        assert!(d2.last_seen_at >= d.last_seen_at);

        let mine: Vec<_> = list_devices()
            .expect("list")
            .into_iter()
            .filter(|x| x.device_id == id)
            .collect();
        assert_eq!(mine.len(), 1, "upsert must not duplicate the device");
        assert_eq!(mine[0].token, t2);
    }

    /// 0189: a new device reads previews ON; `Some(false)` turns it off;
    /// a re-register without the field (`None`, an older phone) keeps it;
    /// `Some(true)` turns it back on.
    #[test]
    fn previews_default_on_and_absent_keeps_the_stored_value() {
        let id = did("previews");
        let d = upsert_device(&id, "apns", &tok("p1"), "owner").expect("insert");
        assert!(d.previews, "a new device defaults to previews on");
        let d = upsert_device_with(&id, "apns", &tok("p2"), "owner", Some(false)).expect("off");
        assert!(!d.previews);
        let d = upsert_device_with(&id, "apns", &tok("p3"), "owner", None).expect("keep");
        assert!(!d.previews, "absent keeps the stored off");
        let d = upsert_device(&id, "apns", &tok("p4"), "owner").expect("keep again");
        assert!(!d.previews, "the plain upsert keeps it too");
        assert!(!get_device(&id).expect("get").expect("row").previews);
        let d = upsert_device_with(&id, "apns", &tok("p5"), "owner", Some(true)).expect("on");
        assert!(d.previews);
        // A brand-new device that says off from the start.
        let id2 = did("previews-off");
        assert!(!upsert_device_with(&id2, "fcm", &tok("p6"), "owner", Some(false)).expect("new off").previews);
        assert!(get_device(&did("never")).expect("get").is_none());
        remove_device(&id).expect("cleanup");
        remove_device(&id2).expect("cleanup");
    }

    #[test]
    fn upsert_validation_is_loud() {
        let err = upsert_device("", "apns", &tok("v"), "owner").expect_err("empty id");
        assert!(err.starts_with("usage:"), "got: {err}");
        let err = upsert_device(&did("v"), "apns", "  ", "owner").expect_err("empty token");
        assert!(err.starts_with("usage:"), "got: {err}");
        let err = upsert_device(&did("v"), "apns", &tok("v"), "").expect_err("empty user");
        assert!(err.starts_with("usage:"), "got: {err}");
        // Platform whitelist: anything outside apns|fcm is refused —
        // including casing (the wire contract is lowercase).
        for bad in ["webpush", "APNS", "Fcm", ""] {
            let err =
                upsert_device(&did("v"), bad, &tok("v"), "owner").expect_err("bad platform");
            assert!(err.contains("platform"), "platform '{bad}' got: {err}");
        }
    }

    #[test]
    fn remove_reports_whether_a_row_existed() {
        let id = did("remove");
        upsert_device(&id, "fcm", &tok("remove"), "owner").expect("insert");
        assert!(remove_device(&id).expect("remove"), "first remove hits");
        assert!(!remove_device(&id).expect("re-remove"), "second remove is a miss");
        assert!(
            !list_devices().expect("list").iter().any(|d| d.device_id == id),
            "removed device must not be listed"
        );
    }

    /// 0195: a register links the row to the caller's session; a later
    /// register re-links it (a new login on the same phone); an owner
    /// register (no session) clears the link; ending a session drops only
    /// the rows linked to it.
    #[test]
    fn session_link_follows_the_latest_register_and_ending_a_session_drops_its_rows() {
        let a = did("sess-a");
        let b = did("sess-b");
        let own = did("sess-owner");
        let sid_a = format!("cu_a{}", uuid::Uuid::new_v4().simple());
        let sid_b = format!("cu_b{}", uuid::Uuid::new_v4().simple());
        let sid_new = format!("cu_n{}", uuid::Uuid::new_v4().simple());
        upsert_device_for_session(&a, "apns", &tok("sa"), "gina", None, Some(&sid_a)).expect("a");
        upsert_device_for_session(&b, "fcm", &tok("sb"), "gina", None, Some(&sid_b)).expect("b");
        upsert_device(&own, "apns", &tok("so"), "owner").expect("owner");
        assert_eq!(device_session_sid(&a).expect("a sid"), Some(sid_a.clone()));
        assert_eq!(device_session_sid(&own).expect("owner sid"), None, "owner token: no session");

        // Same phone, new login: the next register re-links.
        upsert_device_for_session(&a, "apns", &tok("sa2"), "gina", None, Some(&sid_new)).expect("relink");
        assert_eq!(device_session_sid(&a).expect("a sid"), Some(sid_new.clone()));

        let db = crate::db::shared();
        let conn = db.lock();
        assert_eq!(
            remove_devices_for_sessions(&conn, std::slice::from_ref(&sid_a)).expect("old session"),
            0,
            "the old login no longer owns the re-linked phone"
        );
        assert_eq!(remove_devices_for_sessions(&conn, std::slice::from_ref(&sid_new)).expect("end"), 1);
        drop(conn);
        assert!(get_device(&a).expect("get a").is_none(), "its session ended: row gone");
        assert!(get_device(&b).expect("get b").is_some(), "another session of the same user keeps its row");
        assert!(get_device(&own).expect("get owner").is_some(), "owner row untouched");

        // A device that moves to the owner token is no longer tied to a login.
        upsert_device(&b, "fcm", &tok("sb2"), "owner").expect("to owner");
        assert_eq!(device_session_sid(&b).expect("b sid"), None);
        let db = crate::db::shared();
        let conn = db.lock();
        assert_eq!(remove_devices_for_sessions(&conn, std::slice::from_ref(&sid_b)).expect("end b"), 0);
        drop(conn);
        remove_device(&b).expect("cleanup");
        remove_device(&own).expect("cleanup");
    }

    /// The sweep backstop on its own DB (a whole-table sweep): linked rows
    /// go when their session is not live; legacy NULL rows of a Connect
    /// user go when the user has no live session or the row is older than
    /// the session lifetime; owner rows and rows registered at/after the
    /// snapshot stay.
    #[test]
    fn sweep_drops_rows_without_a_live_session_and_keeps_owner_and_fresh_rows() {
        let _db = crate::db::scoped_for_test();
        let ins = |id: &str, user: &str, sid: Option<&str>, seen: i64| {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO push_devices (device_id, platform, token, username, created_at, \
                 updated_at, last_seen_at, session_sid) VALUES (?1, 'apns', ?1, ?2, ?3, ?3, ?3, ?4)",
                params![id, user, seen, sid],
            )
            .expect("insert");
        };
        let now = 10_000;
        let cutoff = now - 7 * 86_400;
        ins("live", "gina", Some("cu_live"), now - 10);
        ins("dead", "gina", Some("cu_dead"), now - 10);
        ins("legacy-recent", "gina", None, now - 10);
        ins("legacy-old", "gina", None, cutoff);
        ins("legacy-nologin", "mo", None, now - 10);
        ins("owner-old", "owner", None, cutoff - 100);
        ins("fresh", "mo", Some("cu_newer_than_snapshot"), now);

        let live: std::collections::HashSet<String> = ["cu_live".to_string()].into();
        let users: std::collections::HashSet<String> = ["gina".to_string()].into();
        let db = crate::db::shared();
        let conn = db.lock();
        let n = prune_devices_without_live_session(&conn, &live, &users, cutoff, now).expect("sweep");
        drop(conn);
        assert_eq!(n, 3, "dead, legacy-old, legacy-nologin");
        let mut left: Vec<String> = list_devices().expect("list").into_iter().map(|d| d.device_id).collect();
        left.sort();
        assert_eq!(left, vec!["fresh", "legacy-recent", "live", "owner-old"]);
    }

    #[test]
    fn prune_by_token_drops_matching_devices_only() {
        let dead = tok("prune-dead");
        let live = tok("prune-live");
        let id_dead = did("prune-dead");
        let id_live = did("prune-live");
        upsert_device(&id_dead, "apns", &dead, "owner").expect("insert dead");
        upsert_device(&id_live, "apns", &live, "owner").expect("insert live");

        assert_eq!(prune_device_by_token(&dead).expect("prune"), 1);
        assert_eq!(prune_device_by_token(&dead).expect("re-prune"), 0);

        let ids: Vec<_> = list_devices()
            .expect("list")
            .into_iter()
            .map(|d| d.device_id)
            .collect();
        assert!(!ids.contains(&id_dead), "dead-token device pruned");
        assert!(ids.contains(&id_live), "unrelated device survives the prune");
        remove_device(&id_live).expect("cleanup");
    }

    /// K5 (0.46.7): a register whose vendor token another row of the same
    /// person already holds replaces that row (the new id stays, with no
    /// leftover key row); different tokens keep both.
    #[test]
    fn a_reregister_with_the_same_token_under_a_new_id_leaves_one_row() {
        let t = tok("dup");
        let old = did("dup-old");
        let new = did("dup-new");
        upsert_device(&old, "apns", &t, "owner").expect("old id");
        let (_, pk) = crate::push::seal::derive_keypair(&[4u8; 32]);
        let key = crate::push_device_keys::NewKey {
            key_id: crate::push::seal::key_id(&pk),
            public_key: pk,
            psk: vec![9u8; 32],
            v: 1,
        };
        upsert_device_e2e(&old, "apns", &t, "owner", None, None, crate::push_device_keys::E2eChange::Set(key))
            .expect("old key");
        assert!(crate::push_device_keys::get_key(&old).expect("key read").is_some());
        upsert_device(&new, "apns", &t, "owner").expect("new id, same token");
        let rows: Vec<_> = list_devices().expect("list").into_iter().filter(|d| d.token == t).collect();
        assert_eq!(rows.len(), 1, "one row per token: {rows:?}");
        assert_eq!(rows[0].device_id, new, "the new id is kept");
        assert!(get_device(&old).expect("get").is_none(), "the old row is gone");
        assert!(crate::push_device_keys::get_key(&old).expect("key read").is_none(), "its key row too");
        // The same id again is a plain refresh, never a self-delete.
        upsert_device(&new, "apns", &t, "owner").expect("refresh");
        assert!(get_device(&new).expect("get").is_some());
        remove_device(&new).expect("cleanup");
    }

    #[test]
    fn different_tokens_keep_two_rows_and_another_platform_is_not_a_duplicate() {
        let (a, b) = (did("two-a"), did("two-b"));
        let (ta, tb) = (tok("two-a"), tok("two-b"));
        upsert_device(&a, "apns", &ta, "owner").expect("a");
        upsert_device(&b, "apns", &tb, "owner").expect("b");
        assert!(get_device(&a).expect("get").is_some());
        assert!(get_device(&b).expect("get").is_some());
        // The same token string on the other vendor is another handle.
        let c = did("two-c");
        upsert_device(&c, "fcm", &ta, "owner").expect("c");
        assert!(get_device(&a).expect("get").is_some(), "an apns row is not an fcm row's duplicate");
        assert!(get_device(&c).expect("get").is_some());
        for id in [&a, &b, &c] {
            remove_device(id).expect("cleanup");
        }
    }

    /// Two people on one phone (two saved server entries, one per login)
    /// share its token: each keeps their own registration.
    #[test]
    fn the_same_token_for_another_person_is_kept() {
        let t = tok("two-people");
        let (a, b) = (did("tp-owner"), did("tp-gina"));
        upsert_device(&a, "apns", &t, "owner").expect("owner");
        upsert_device(&b, "apns", &t, "k5gina").expect("gina");
        assert!(get_device(&a).expect("get").is_some(), "the owner's row stays");
        assert!(get_device(&b).expect("get").is_some());
        remove_device(&a).expect("cleanup");
        remove_device(&b).expect("cleanup");
    }

    #[test]
    fn the_token_fingerprint_is_stable_short_hex_and_never_the_token() {
        let t = "80f2c1d3e4a5b6c7d8e9f0a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1";
        let f = token_fingerprint(t);
        assert_eq!(f, token_fingerprint(t), "stable");
        assert_eq!(f.len(), 8);
        assert!(f.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "{f}");
        assert!(!t.contains(&f), "the fingerprint is not a slice of the token: {f}");
        // sha256("abc") = ba7816bf…: the first 4 bytes, pinned.
        assert_eq!(token_fingerprint("abc"), "ba7816bf");
        assert_ne!(token_fingerprint("abd"), token_fingerprint("abc"));
    }

    #[test]
    fn dedupe_by_token_keeps_the_most_recently_seen_row() {
        let row = |id: &str, token: &str, seen: i64| PushDevice {
            device_id: id.into(),
            platform: "apns".into(),
            token: token.into(),
            username: "owner".into(),
            created_at: 1,
            updated_at: seen,
            last_seen_at: seen,
            previews: true,
        };
        let out = dedupe_by_token(&[row("old", "t1", 10), row("other", "t2", 5), row("new", "t1", 20)]);
        let ids: Vec<_> = out.iter().map(|d| d.device_id.as_str()).collect();
        assert_eq!(ids, ["other", "new"]);
    }
}
