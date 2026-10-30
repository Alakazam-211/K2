//! Per-device push keys (prd-e2e-push-v1 rev 2 §7.1, §8; migration 0211).
//!
//! A phone on the new K2 Companion registers a P-256 public key and a
//! 32-byte PSK with this server over the E2E path (never through the
//! relay). While a device has an active key, the server can seal its
//! pushes ([`crate::push::seal`]). A device without one keeps today's
//! pushes, byte for byte.
//!
//! Ownership (V12): a register that changes the device's login deletes the
//! key row in the same transaction ([`crate::push_devices::upsert_device_e2e`]).
//! Deletes (V13): every path that deletes a `push_devices` row deletes the
//! key row explicitly; the foreign key (`ON DELETE CASCADE`) is a second
//! guard.
//!
//! The public key and PSK are never echoed by any route; only `keyId` is.

use base64::Engine as _;
use rusqlite::{params, Connection, OptionalExtension};

use crate::push::seal;

/// Keys expire 60 days after the server received them; the device is then
/// treated as unkeyed.
pub const KEY_TTL_SECS: i64 = 60 * 86_400;
/// The phone makes and registers a new key after 30 days.
pub const ROTATE_AFTER_SECS: i64 = 30 * 86_400;

/// The `e2e` object a phone sends on `POST /cli/push/register-device`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct E2eRegistration {
    /// The sealed format the phone can open: 1 (every phone), or 2 (also
    /// the bigger buckets, push-e2e-v2, 0.46.7). A daemon without
    /// `push-e2e-v2` in `/boot-status` refuses 2.
    pub v: u32,
    pub suite: String,
    /// base64url of the 65-byte SEC1 uncompressed public key.
    pub public_key: String,
    /// base64url of the 32-byte PSK.
    pub psk: String,
    /// The phone's own creation time (informational).
    #[serde(default)]
    pub created_at: Option<i64>,
}

/// A validated key, ready to store.
#[derive(Clone, PartialEq, Eq)]
pub struct NewKey {
    pub public_key: Vec<u8>,
    pub psk: Vec<u8>,
    pub key_id: String,
    /// The registration's `e2e.v` (1 or 2).
    pub v: u32,
}

impl std::fmt::Debug for NewKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the PSK.
        f.debug_struct("NewKey").field("key_id", &self.key_id).finish_non_exhaustive()
    }
}

/// What a register does to the stored key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum E2eChange {
    /// `e2e` absent: keep the stored key (only if the login is unchanged).
    Keep,
    /// `e2e: null`: delete it.
    Delete,
    /// A key: store it (the same key again is a no-op).
    Set(NewKey),
}

/// The owner's per-device override (`push_devices.e2e_override`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceOverride {
    /// Today's path for this device, whatever the server switch says.
    Off,
    /// Sealed when the device has a key and the relay supports it.
    Sealed,
}

impl DeviceOverride {
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Sealed => "sealed",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "off" => Some(Self::Off),
            "sealed" => Some(Self::Sealed),
            _ => None,
        }
    }
}

/// A stored key row.
#[derive(Clone, PartialEq, Eq)]
pub struct StoredKey {
    pub device_id: String,
    pub suite: String,
    pub public_key: Vec<u8>,
    pub psk: Vec<u8>,
    pub key_id: String,
    pub next_ctr: i64,
    pub created_at: i64,
    pub expires_at: i64,
    pub updated_at: i64,
    /// The highest sealed format the phone can open (migration 0252).
    pub v: u32,
}

impl std::fmt::Debug for StoredKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredKey")
            .field("device_id", &self.device_id)
            .field("key_id", &self.key_id)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

impl StoredKey {
    pub fn is_active(&self, now: i64) -> bool {
        now < self.expires_at
    }

    pub fn rotate_after(&self) -> i64 {
        self.created_at + ROTATE_AFTER_SECS
    }

    /// The sealed format to use for this phone: 2 only when the phone can
    /// open it AND the relay accepts it (`relay_v2`).
    pub fn format(&self, relay_v2: bool) -> seal::Format {
        if self.v >= 2 && relay_v2 {
            seal::Format::V2
        } else {
            seal::Format::V1
        }
    }
}

/// One device's E2E state for dispatch and the routes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceE2e {
    pub override_: Option<DeviceOverride>,
    pub key: Option<StoredKey>,
}

/// base64url, with or without padding.
fn b64url_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim().trim_end_matches('=');
    seal::B64.decode(s).ok()
}

/// Validate a registration (V14: a usage error names the field).
pub fn validate(reg: &E2eRegistration) -> Result<NewKey, String> {
    if !(1..=2).contains(&reg.v) {
        return Err(format!("usage: e2e.v must be 1 or 2 (got {})", reg.v));
    }
    if reg.suite != seal::SUITE_NAME {
        return Err(format!("usage: e2e.suite must be '{}' (got '{}')", seal::SUITE_NAME, reg.suite));
    }
    let public_key = b64url_decode(&reg.public_key)
        .ok_or_else(|| "usage: e2e.publicKey must be base64url".to_string())?;
    seal::check_public_key(&public_key).map_err(|e| format!("usage: e2e.publicKey: {e}"))?;
    let psk = b64url_decode(&reg.psk).ok_or_else(|| "usage: e2e.psk must be base64url".to_string())?;
    if psk.len() != seal::PSK_LEN {
        return Err(format!("usage: e2e.psk must be {} bytes (got {})", seal::PSK_LEN, psk.len()));
    }
    if psk.iter().all(|b| *b == 0) {
        return Err("usage: e2e.psk must be random bytes, not zeros".to_string());
    }
    let key_id = seal::key_id(&public_key);
    Ok(NewKey { public_key, psk, key_id, v: reg.v })
}

const KEY_SELECT: &str = "SELECT device_id, suite, public_key, psk, key_id, next_ctr, created_at, \
    expires_at, updated_at, v FROM push_device_keys";

fn row_to_key(r: &rusqlite::Row) -> rusqlite::Result<StoredKey> {
    Ok(StoredKey {
        device_id: r.get(0)?,
        suite: r.get(1)?,
        public_key: r.get(2)?,
        psk: r.get(3)?,
        key_id: r.get(4)?,
        next_ctr: r.get(5)?,
        created_at: r.get(6)?,
        expires_at: r.get(7)?,
        updated_at: r.get(8)?,
        v: r.get::<_, i64>(9).map(|v| u32::try_from(v).unwrap_or(1))?,
    })
}

/// The stored key of a device (expired or not).
pub fn get_key_in(conn: &Connection, device_id: &str) -> Result<Option<StoredKey>, String> {
    conn.query_row(&format!("{KEY_SELECT} WHERE device_id = ?1"), params![device_id.trim()], row_to_key)
        .optional()
        .map_err(|e| format!("push key read failed: {e}"))
}

/// [`get_key_in`] on the shared DB.
pub fn get_key(device_id: &str) -> Result<Option<StoredKey>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    get_key_in(&conn, device_id)
}

/// Store a key for a device. The same public key and PSK again is a no-op
/// (`Ok(false)`) apart from its `v` (an app update that learned format 2
/// re-registers the same key with `v: 2`); a new key replaces the old one,
/// which is not kept.
pub fn put_key_in(conn: &Connection, device_id: &str, key: &NewKey, now: i64) -> Result<bool, String> {
    if let Some(old) = get_key_in(conn, device_id)? {
        if old.public_key == key.public_key && old.psk == key.psk {
            if old.v != key.v {
                conn.execute(
                    "UPDATE push_device_keys SET v = ?2 WHERE device_id = ?1",
                    params![device_id.trim(), i64::from(key.v)],
                )
                .map_err(|e| format!("push key format update failed: {e}"))?;
            }
            return Ok(false);
        }
    }
    conn.execute(
        "INSERT INTO push_device_keys (device_id, suite, public_key, psk, key_id, next_ctr, \
         created_at, expires_at, updated_at, v) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7, ?6, ?8) \
         ON CONFLICT(device_id) DO UPDATE SET suite = excluded.suite, \
         public_key = excluded.public_key, psk = excluded.psk, key_id = excluded.key_id, \
         created_at = excluded.created_at, expires_at = excluded.expires_at, \
         updated_at = excluded.updated_at, v = excluded.v",
        params![
            device_id.trim(),
            seal::SUITE_NAME,
            key.public_key,
            key.psk,
            key.key_id,
            now,
            now + KEY_TTL_SECS,
            i64::from(key.v)
        ],
    )
    .map_err(|e| format!("push key store failed: {e}"))?;
    Ok(true)
}

/// Delete a device's key row. Returns the rows removed.
pub fn delete_key_in(conn: &Connection, device_id: &str) -> Result<usize, String> {
    conn.execute("DELETE FROM push_device_keys WHERE device_id = ?1", params![device_id.trim()])
        .map_err(|e| format!("push key delete failed: {e}"))
}

/// Take the next replay counter for a device key, in one atomic
/// `UPDATE … RETURNING`: `ctr = max(next_ctr, now_ms)`, and `next_ctr`
/// becomes `ctr + 1`. A restored backup can't rewind it (the clock wins).
pub fn take_ctr_in(conn: &Connection, device_id: &str, now_ms: i64) -> Result<u64, String> {
    let ctr: i64 = conn
        .query_row(
            "UPDATE push_device_keys SET next_ctr = MAX(next_ctr, ?2) + 1 WHERE device_id = ?1 \
             RETURNING next_ctr - 1",
            params![device_id.trim(), now_ms],
            |r| r.get(0),
        )
        .map_err(|e| format!("push key counter failed: {e}"))?;
    u64::try_from(ctr).map_err(|_| "push key counter is negative".to_string())
}

/// [`take_ctr_in`] on the shared DB.
pub fn take_ctr(device_id: &str, now_ms: i64) -> Result<u64, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    take_ctr_in(&conn, device_id, now_ms)
}

/// Set (`Some`) or clear (`None`) a device's override. `Ok(false)` when
/// the device is unknown.
pub fn set_override(device_id: &str, value: Option<DeviceOverride>) -> Result<bool, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let n = conn
        .execute(
            "UPDATE push_devices SET e2e_override = ?2 WHERE device_id = ?1",
            params![device_id.trim(), value.map(DeviceOverride::as_wire)],
        )
        .map_err(|e| format!("push device override failed: {e}"))?;
    Ok(n > 0)
}

/// The E2E state of these devices (override + key), by device id. Devices
/// without a row are absent.
pub fn states(device_ids: &[&str]) -> Result<std::collections::HashMap<String, DeviceE2e>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let mut out = std::collections::HashMap::new();
    for id in device_ids {
        let ov: Option<Option<String>> = conn
            .query_row("SELECT e2e_override FROM push_devices WHERE device_id = ?1", params![id], |r| r.get(0))
            .optional()
            .map_err(|e| format!("push device override read failed: {e}"))?;
        let Some(ov) = ov else { continue };
        let key = get_key_in(&conn, id)?;
        out.insert(
            id.to_string(),
            DeviceE2e { override_: ov.as_deref().and_then(DeviceOverride::from_wire), key },
        );
    }
    Ok(out)
}

/// One device's state ([`states`] for one id); `Default` when unknown.
pub fn state(device_id: &str) -> Result<DeviceE2e, String> {
    Ok(states(&[device_id])?.remove(device_id).unwrap_or_default())
}

/// Backups (V17): empty `push_device_keys` in a COPY of `k2.db` before it
/// leaves the box. Phones re-register after a restore. Returns the rows
/// removed. Never call this on the live database.
pub fn scrub_snapshot(conn: &Connection) -> Result<usize, String> {
    conn.execute("DELETE FROM push_device_keys", [])
        .map_err(|e| format!("push key scrub failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::push_devices::{self as devices, upsert_device, upsert_device_e2e, upsert_device_for_session};

    pub(crate) fn test_key(seed: u8) -> NewKey {
        let (_, pk) = seal::derive_keypair(&[seed; 32]);
        let psk = vec![seed.wrapping_add(1); 32];
        NewKey { key_id: seal::key_id(&pk), public_key: pk, psk, v: 1 }
    }

    fn reg_of(k: &NewKey) -> E2eRegistration {
        E2eRegistration {
            v: 1,
            suite: seal::SUITE_NAME.into(),
            public_key: seal::B64.encode(&k.public_key),
            psk: seal::B64.encode(&k.psk),
            created_at: Some(1),
        }
    }

    fn key_rows(conn: &Connection, id: &str) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM push_device_keys WHERE device_id = ?1", params![id], |r| r.get(0))
            .expect("count")
    }

    fn fk_on(conn: &Connection) -> i64 {
        conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0)).expect("pragma")
    }

    fn keyed(id: &str, user: &str, sid: Option<&str>, seed: u8) {
        upsert_device_e2e(id, "apns", &format!("tok-{id}"), user, None, sid, E2eChange::Set(test_key(seed)))
            .expect("register with key");
    }

    /// push-e2e-v2: the same key again with `v: 2` (an app update) keeps
    /// the key row and changes only its format; format 2 is used only with
    /// a relay that accepts it.
    #[test]
    fn the_same_key_again_with_v2_updates_only_its_format() {
        let id = format!("kv2-{}", uuid::Uuid::new_v4());
        let tok = format!("tok-{id}");
        upsert_device_e2e(&id, "apns", &tok, "owner", None, None, E2eChange::Set(test_key(21))).expect("v1");
        let before = get_key(&id).expect("read").expect("key");
        assert_eq!(before.v, 1);
        assert_eq!(before.format(true), seal::Format::V1, "a v1 phone never gets format 2");
        let mut k = test_key(21);
        k.v = 2;
        upsert_device_e2e(&id, "apns", &tok, "owner", None, None, E2eChange::Set(k)).expect("v2");
        let after = get_key(&id).expect("read").expect("key");
        assert_eq!(after.v, 2);
        assert_eq!(after.key_id, before.key_id);
        assert_eq!(after.created_at, before.created_at, "the same key is not re-created");
        assert_eq!(after.format(true), seal::Format::V2);
        assert_eq!(after.format(false), seal::Format::V1, "a relay without s2 keeps format 1");
        devices::remove_device(&id).expect("cleanup");
    }

    #[test]
    fn validation_names_the_field() {
        let k = test_key(4);
        assert_eq!(validate(&reg_of(&k)).expect("valid"), k);
        // push-e2e-v2 (0.46.7): 2 is a phone that opens the bigger buckets.
        let mut r = reg_of(&k);
        r.v = 2;
        assert_eq!(validate(&r).expect("v2").v, 2);
        for bad in [0, 3, 9] {
            let mut r = reg_of(&k);
            r.v = bad;
            assert!(validate(&r).expect_err("v").contains("e2e.v"), "v={bad}");
        }
        let mut r = reg_of(&k);
        r.suite = "p256-base".into();
        assert!(validate(&r).expect_err("suite").contains("e2e.suite"));
        let mut r = reg_of(&k);
        r.public_key = seal::B64.encode(&k.public_key[..64]);
        assert!(validate(&r).expect_err("short").contains("e2e.publicKey"));
        let mut off = k.public_key.clone();
        off[64] ^= 1;
        let mut r = reg_of(&k);
        r.public_key = seal::B64.encode(&off);
        assert!(validate(&r).expect_err("off curve").contains("e2e.publicKey"));
        let mut r = reg_of(&k);
        r.psk = seal::B64.encode([1u8; 16]);
        assert!(validate(&r).expect_err("psk").contains("e2e.psk"));
        let mut r = reg_of(&k);
        r.psk = seal::B64.encode([0u8; 32]);
        assert!(validate(&r).expect_err("zero psk").contains("e2e.psk"));
        let mut r = reg_of(&k);
        r.psk = "!!".into();
        assert!(validate(&r).expect_err("b64").contains("e2e.psk"));
        // Padded base64url is accepted too.
        let mut r = reg_of(&k);
        r.public_key = base64::engine::general_purpose::URL_SAFE.encode(&k.public_key);
        assert!(validate(&r).is_ok());
    }

    #[test]
    fn keep_delete_set_and_same_key_is_a_no_op() {
        let _db = crate::db::scoped_for_test();
        keyed("k-1", "owner", None, 1);
        let first = get_key("k-1").expect("get").expect("row");
        assert_eq!(first.key_id, test_key(1).key_id);
        assert_eq!(first.expires_at - first.created_at, KEY_TTL_SECS);
        // A counter advance, then the same key again: the row (and counter) stays.
        let c = take_ctr("k-1", 5).expect("ctr");
        upsert_device_e2e("k-1", "apns", "tok-k-1", "owner", None, None, E2eChange::Set(test_key(1))).expect("same");
        assert_eq!(get_key("k-1").expect("get").expect("row").next_ctr, c as i64 + 1, "same key: no-op");
        // Absent keeps it.
        upsert_device("k-1", "apns", "tok-k-1b", "owner").expect("plain register");
        assert!(get_key("k-1").expect("get").is_some(), "absent e2e keeps the key");
        // A new key replaces it; the old one isn't kept.
        upsert_device_e2e("k-1", "apns", "tok-k-1", "owner", None, None, E2eChange::Set(test_key(2))).expect("new");
        assert_eq!(get_key("k-1").expect("get").expect("row").key_id, test_key(2).key_id);
        // null deletes it.
        upsert_device_e2e("k-1", "apns", "tok-k-1", "owner", None, None, E2eChange::Delete).expect("null");
        assert!(get_key("k-1").expect("get").is_none());
    }

    /// V12: a register from another login takes over the row and must drop
    /// the old person's key in the same transaction; the new login's key
    /// (if sent) is stored fresh.
    #[test]
    fn a_new_login_on_the_same_device_drops_the_old_key() {
        let _db = crate::db::scoped_for_test();
        keyed("own-1", "gina", Some("cu_gina"), 1);
        // Plain register by mo (old app, no e2e): gina's key must go.
        upsert_device_for_session("own-1", "apns", "tok-own-1", "mo", None, Some("cu_mo")).expect("takeover");
        assert!(get_key("own-1").expect("get").is_none(), "gina's key must not stay attached to mo's row");
        // Takeover WITH a key: the new key is stored, not the old one.
        keyed("own-2", "gina", Some("cu_gina"), 1);
        keyed("own-2", "mo", Some("cu_mo"), 3);
        assert_eq!(get_key("own-2").expect("get").expect("row").key_id, test_key(3).key_id);
        // Same login re-registering without e2e keeps its key.
        upsert_device_for_session("own-2", "apns", "tok-own-2", "mo", None, Some("cu_mo2")).expect("same login");
        assert!(get_key("own-2").expect("get").is_some());
    }

    #[test]
    fn take_ctr_is_monotonic_and_can_not_be_rewound() {
        let _db = crate::db::scoped_for_test();
        keyed("c-1", "owner", None, 1);
        let a = take_ctr("c-1", 1_000).expect("a");
        assert_eq!(a, 1_000, "clock above next_ctr");
        let b = take_ctr("c-1", 1_000).expect("b");
        assert_eq!(b, 1_001, "same ms: next_ctr wins");
        let c = take_ctr("c-1", 10).expect("c");
        assert_eq!(c, 1_002, "a clock behind can't rewind");
        // A restored backup rewinds next_ctr; the clock still keeps it ahead.
        {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.execute("UPDATE push_device_keys SET next_ctr = 1 WHERE device_id = 'c-1'", []).expect("rewind");
        }
        assert_eq!(take_ctr("c-1", 5_000).expect("d"), 5_000);
        assert!(take_ctr("missing", 1).is_err(), "no key row: loud error");
    }

    #[test]
    fn overrides_and_states() {
        let _db = crate::db::scoped_for_test();
        keyed("s-1", "owner", None, 1);
        upsert_device("s-2", "fcm", "tok-s-2", "owner").expect("plain");
        assert!(set_override("s-1", Some(DeviceOverride::Off)).expect("set"));
        assert!(!set_override("nope", Some(DeviceOverride::Sealed)).expect("unknown"));
        let st = states(&["s-1", "s-2", "nope"]).expect("states");
        assert_eq!(st["s-1"].override_, Some(DeviceOverride::Off));
        assert!(st["s-1"].key.is_some());
        assert_eq!(st["s-2"], DeviceE2e::default());
        assert!(!st.contains_key("nope"));
        assert!(set_override("s-1", None).expect("clear"));
        assert_eq!(state("s-1").expect("state").override_, None);
    }

    /// V13: all five delete paths delete the key row EXPLICITLY. Each runs
    /// with foreign keys OFF (as after a failed PRAGMA or self-heal), so
    /// only the explicit delete can pass; then with them ON the FK cascade
    /// is checked as the second guard.
    #[test]
    fn all_five_delete_paths_drop_the_key_row() {
        let _db = crate::db::scoped_for_test();
        let set_fk = |on: bool| {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.execute_batch(if on { "PRAGMA foreign_keys = ON;" } else { "PRAGMA foreign_keys = OFF;" })
                .expect("pragma");
            assert_eq!(fk_on(&conn), i64::from(on));
        };
        let gone = |id: &str| {
            let db = crate::db::shared();
            let conn = db.lock();
            assert_eq!(key_rows(&conn, id), 0, "{id}: key row must be gone");
            assert!(devices::get_device(id).expect("get").is_none(), "{id}: device row gone");
        };
        set_fk(false);
        // 1. remove_device
        keyed("d-remove", "owner", None, 1);
        assert!(devices::remove_device("d-remove").expect("remove"));
        gone("d-remove");
        // 2. remove_devices_for_sessions
        keyed("d-session", "gina", Some("cu_end"), 2);
        {
            let db = crate::db::shared();
            let conn = db.lock();
            assert_eq!(devices::remove_devices_for_sessions(&conn, &["cu_end".to_string()]).expect("end"), 1);
        }
        gone("d-session");
        // 3. prune_device_by_token
        keyed("d-token", "owner", None, 3);
        assert_eq!(devices::prune_device_by_token("tok-d-token").expect("prune"), 1);
        gone("d-token");
        // 4. prune_devices_without_live_session (registered before the snapshot)
        keyed("d-sweep", "gina", Some("cu_dead"), 4);
        {
            let db = crate::db::shared();
            let conn = db.lock();
            let now = i64::MAX;
            let n = devices::prune_devices_without_live_session(
                &conn,
                &Default::default(),
                &Default::default(),
                0,
                now,
            )
            .expect("sweep");
            assert_eq!(n, 1);
        }
        gone("d-sweep");
        // 5. notify_prefs::cleanup_removed_user
        keyed("d-user", "zed", Some("cu_zed"), 5);
        {
            let db = crate::db::shared();
            let conn = db.lock();
            crate::notify_prefs::cleanup_removed_user(&conn, "zed").expect("cleanup");
        }
        gone("d-user");
        // The FK cascade as the second guard: a raw delete of the device row.
        set_fk(true);
        keyed("d-fk", "owner", None, 6);
        {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.execute("DELETE FROM push_devices WHERE device_id = 'd-fk'", []).expect("raw delete");
            assert_eq!(key_rows(&conn, "d-fk"), 0, "ON DELETE CASCADE");
        }
    }

    #[test]
    fn scrub_snapshot_empties_the_copy() {
        let _db = crate::db::scoped_for_test();
        keyed("b-1", "owner", None, 1);
        keyed("b-2", "owner", None, 2);
        let db = crate::db::shared();
        let conn = db.lock();
        assert_eq!(scrub_snapshot(&conn).expect("scrub"), 2);
        assert!(devices::get_device("b-1").expect("get").is_some(), "only the key rows go");
    }

    #[test]
    fn debug_never_prints_the_psk_or_key() {
        let k = test_key(9);
        let s = format!("{k:?}");
        assert!(!s.contains(&format!("{:?}", k.psk)));
        assert!(s.contains(&k.key_id));
    }
}
