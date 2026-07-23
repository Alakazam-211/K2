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
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

const DEVICE_SELECT: &str = "SELECT device_id, platform, token, username, \
    created_at, updated_at, last_seen_at FROM push_devices";

fn row_to_device(row: &rusqlite::Row) -> rusqlite::Result<PushDevice> {
    Ok(PushDevice {
        device_id: row.get(0)?,
        platform: row.get(1)?,
        token: row.get(2)?,
        username: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
        last_seen_at: row.get(6)?,
    })
}

/// Register (or refresh) a device. Keyed by `device_id`: a re-register
/// updates platform/token/username and bumps `updated_at` +
/// `last_seen_at`, preserving `created_at` — the companion re-sends on
/// every app launch, which is also how APNs/FCM token rotation lands.
pub fn upsert_device(
    device_id: &str,
    platform: &str,
    token: &str,
    username: &str,
) -> Result<PushDevice, String> {
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
    let db = crate::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO push_devices (device_id, platform, token, username, \
         created_at, updated_at, last_seen_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?5) \
         ON CONFLICT(device_id) DO UPDATE SET \
         platform = excluded.platform, token = excluded.token, \
         username = excluded.username, updated_at = excluded.updated_at, \
         last_seen_at = excluded.last_seen_at",
        params![device_id, platform, token, username, now],
    )
    .map_err(|e| format!("push device upsert failed: {e}"))?;
    conn.query_row(
        &format!("{DEVICE_SELECT} WHERE device_id = ?1"),
        params![device_id],
        row_to_device,
    )
    .map_err(|e| format!("push device readback failed: {e}"))
}

/// Unregister a device by its id. `Ok(true)` when a row was removed,
/// `Ok(false)` when the id was unknown (an unregister of an already-
/// gone device is a fine no-op — the companion toggles blind).
pub fn remove_device(device_id: &str) -> Result<bool, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let n = conn
        .execute(
            "DELETE FROM push_devices WHERE device_id = ?1",
            params![device_id.trim()],
        )
        .map_err(|e| format!("push device delete failed: {e}"))?;
    Ok(n > 0)
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

/// Drop every device holding a vendor token the gateway reported dead
/// (`410 Unregistered` / FCM `NotRegistered` → `dead:true` in the
/// dispatch response). Returns the number of rows pruned. Keyed by
/// TOKEN — that's all the gateway knows.
pub fn prune_device_by_token(token: &str) -> Result<usize, String> {
    let db = crate::db::shared();
    let conn = db.lock();
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
}
