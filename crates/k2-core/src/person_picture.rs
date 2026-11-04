//! Profile pictures for K2's own people (0.46.8, Rosson 2026-11-04): the
//! owner and every Connect login on this server. App users have theirs in
//! skin.db (`skin::profile`); agents have `k2 agent identity`.
//!
//! - **Per server.** Each server keeps its own picture for each person
//!   (Rosson 2026-11-04). Nothing here syncs from K2 Connect or other
//!   servers, or pushes to them.
//! - **Keyed by the stable id.** The owner person is `owner` (the owner
//!   token and every owner-linked login); a Connect login is
//!   `connect:<id>`, its never-reused account id
//!   ([`crate::connect_users::user_uuid`]), so a removed name added again
//!   never inherits the old picture. Removing a login drops its row.
//! - **One image pipeline.** The bytes are K2's own re-encode from
//!   [`crate::workspace::agent_look::normalize_profile_image`] (the App
//!   user profile's: PNG/JPEG/WebP in, at most 2 MiB and 4096 px a side,
//!   EXIF orientation applied then dropped, always JPEG out, at most
//!   64 KiB). The upload's bytes are never stored or served.
//! - **Store.** k2.db `person_pictures` (migration 0264).

use std::collections::BTreeMap;

use rusqlite::{params, Connection, OptionalExtension};

use crate::notify_prefs::{People, OWNER};

/// The storage key of the owner person.
pub const OWNER_KEY: &str = OWNER;
/// A Connect login's storage key prefix (then its account id).
pub const CONNECT_PREFIX: &str = "connect:";

/// One stored picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    /// K2's own JPEG.
    pub jpeg: Vec<u8>,
    /// SHA-256 hex of [`Stored::jpeg`].
    pub hash: String,
    /// Unix seconds.
    pub set_at: i64,
}

impl Stored {
    /// The picture as a `data:image/jpeg;base64,…` URL.
    pub fn data_url(&self) -> String {
        use base64::Engine as _;
        format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(&self.jpeg))
    }
}

/// The storage key for a person key (`owner`, or a Connect username).
/// `Ok(None)` when the username is not a Connect login here.
pub fn storage_key(person: &str) -> Result<Option<String>, String> {
    if person == OWNER {
        return Ok(Some(OWNER_KEY.to_string()));
    }
    Ok(crate::connect_users::user_uuid(person)?.filter(|id| !id.is_empty()).map(|id| format!("{CONNECT_PREFIX}{id}")))
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Store `jpeg` (already K2's re-encode) for `key`, replacing any older
/// picture. Returns its hash.
pub fn set(conn: &Connection, key: &str, jpeg: &[u8], by: &str) -> Result<String, String> {
    if jpeg.is_empty() || jpeg.len() > crate::workspace::agent_look::PROFILE_MAX_BYTES {
        return Err(format!("person picture: {} bytes is not a stored picture", jpeg.len()));
    }
    let hash = crate::skin::profile::image_hash(jpeg);
    conn.execute(
        "INSERT INTO person_pictures (person_key, image, image_type, image_hash, set_at, set_by)
         VALUES (?1, ?2, 'image/jpeg', ?3, ?4, ?5)
         ON CONFLICT(person_key) DO UPDATE SET image = excluded.image, image_hash = excluded.image_hash,
           set_at = excluded.set_at, set_by = excluded.set_by",
        params![key, jpeg, hash, now(), by],
    )
    .map_err(|e| format!("person picture: {e}"))?;
    Ok(hash)
}

/// Drop `key`'s picture. True when there was one.
pub fn clear(conn: &Connection, key: &str) -> Result<bool, String> {
    conn.execute("DELETE FROM person_pictures WHERE person_key = ?1", params![key])
        .map(|n| n > 0)
        .map_err(|e| format!("person picture clear: {e}"))
}

/// `key`'s picture, if any.
pub fn get(conn: &Connection, key: &str) -> Result<Option<Stored>, String> {
    conn.query_row(
        "SELECT image, image_hash, set_at FROM person_pictures WHERE person_key = ?1",
        params![key],
        |r| Ok(Stored { jpeg: r.get(0)?, hash: r.get(1)?, set_at: r.get(2)? }),
    )
    .optional()
    .map_err(|e| format!("person picture: {e}"))
}

/// Storage key → hash for every stored picture.
pub fn hashes(conn: &Connection) -> Result<BTreeMap<String, String>, String> {
    let mut stmt = conn
        .prepare("SELECT person_key, image_hash FROM person_pictures")
        .map_err(|e| format!("person pictures: {e}"))?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| format!("person pictures: {e}"))?;
    rows.collect::<Result<BTreeMap<_, _>, _>>().map_err(|e| format!("person pictures: {e}"))
}

/// A removed Connect login's picture goes with it.
pub fn remove_for_user_id(conn: &Connection, id: &str) -> Result<bool, String> {
    if id.is_empty() {
        return Ok(false);
    }
    clear(conn, &format!("{CONNECT_PREFIX}{id}"))
}

/// One person with a picture, as `GET /cli/people/picture` lists it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Listed {
    /// The person key: `owner`, or a Connect username.
    pub person: String,
    /// Every login that shows this picture (`owner` plus the owner-linked
    /// logins, or the one username).
    pub logins: Vec<String>,
    #[serde(rename = "imageHash")]
    pub image_hash: String,
}

/// Every person with a picture on this server, owner first, then by
/// username. `ids` maps a Connect username to its account id.
pub fn list(conn: &Connection, people: &People, ids: &BTreeMap<String, String>) -> Result<Vec<Listed>, String> {
    let stored = hashes(conn)?;
    let mut out = Vec::new();
    if let Some(h) = stored.get(OWNER_KEY) {
        let mut logins = vec![OWNER.to_string()];
        logins.extend(people.owner_aliases());
        out.push(Listed { person: OWNER.to_string(), logins, image_hash: h.clone() });
    }
    let mut users: Vec<&crate::connect_users::ConnectUserView> = people.users.iter().collect();
    users.sort_by(|a, b| a.username.cmp(&b.username));
    for u in users {
        // Owner-linked logins show the owner's; a reserved name is nobody.
        if people.login_person_key(&u.username).is_none_or(|p| p == OWNER) {
            continue;
        }
        let Some(id) = ids.get(&u.username) else { continue };
        if let Some(h) = stored.get(&format!("{CONNECT_PREFIX}{id}")) {
            out.push(Listed { person: u.username.clone(), logins: vec![u.username.clone()], image_hash: h.clone() });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().expect("open");
        crate::db::run_migrations(&c).expect("migrate");
        c
    }

    #[test]
    fn set_replace_get_and_clear() {
        let c = conn();
        assert_eq!(get(&c, "owner").expect("get"), None);
        let h1 = set(&c, "owner", b"\xff\xd8one", "owner").expect("set");
        let h2 = set(&c, "owner", b"\xff\xd8two", "owner").expect("replace");
        assert_ne!(h1, h2, "a new picture has a new hash");
        let got = get(&c, "owner").expect("get").expect("stored");
        assert_eq!(got.jpeg, b"\xff\xd8two");
        assert_eq!(got.hash, h2);
        assert!(got.data_url().starts_with("data:image/jpeg;base64,"));
        assert_eq!(hashes(&c).expect("hashes").len(), 1, "a replace keeps one row");
        assert!(clear(&c, "owner").expect("clear"));
        assert!(!clear(&c, "owner").expect("clear again"), "a second clear finds nothing");
        assert_eq!(get(&c, "owner").expect("get"), None);
    }

    #[test]
    fn an_empty_or_oversized_picture_is_refused() {
        let c = conn();
        assert!(set(&c, "owner", b"", "owner").is_err());
        let big = vec![0u8; crate::workspace::agent_look::PROFILE_MAX_BYTES + 1];
        assert!(set(&c, "owner", &big, "owner").is_err());
        assert_eq!(hashes(&c).expect("hashes").len(), 0);
    }

    #[test]
    fn a_removed_login_loses_its_row() {
        let c = conn();
        set(&c, "connect:abc", b"\xff\xd8x", "owner").expect("set");
        assert!(remove_for_user_id(&c, "abc").expect("remove"));
        assert!(!remove_for_user_id(&c, "").expect("empty id"), "an empty id matches nothing");
        assert_eq!(hashes(&c).expect("hashes").len(), 0);
    }
}
